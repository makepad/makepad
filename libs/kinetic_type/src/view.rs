//! A kit running: [`KineticView`] loads a kit, builds the glyph set for a
//! text, and each frame fills the records, runs the animator kernel,
//! buckets the records by shape and draws them into its HDR target with
//! the kit's camera, backdrop and floor, then the kit's post passes (the
//! render graph). The host shows the returned texture (a Motion layer, a
//! game widget) or reads it as a picture (the text texture).

use crate::draw::{DrawKineticBackdrop, DrawKineticGlyph};
use crate::kernel::{self, SIGNALS};
use crate::kit::{self, KitValues, Split};
use crate::records::{Karaoke, Records, GLYPH_WORDS};
use crate::shapes::{self, GlyphSet, VERT_FLOATS};
use makepad_draw::makepad_platform::draw_shader_layout::{LayoutKind, LayoutPacking, PodType};
use makepad_draw::*;
use makepad_audio_reactive::{bind_audio, AudioFrame};
use makepad_render_graph::{Attachments, FrameUniforms, GraphRunner, Stage, StageInputs};
use makepad_script_compute::kernel::{FieldTy, Kernel, Layout, LayoutField};
use std::sync::Arc;

/// The per-frame signals a host gives.
#[derive(Clone, Default)]
pub struct KineticFrame {
    /// Seconds (the layer's time).
    pub time: f32,
    /// Fractional beats; with no music the host passes `time * bpm / 60`.
    pub beat: f32,
    pub bpm: f32,
    pub energy: f32,
    /// Bass, mid, high (0..1).
    pub bands: [f32; 3],
    /// The first four dials (p1..p4) when the host sets them.
    pub dials: [Option<f32>; 4],
    pub karaoke: Karaoke,
    /// The picture under the layer (glass, backdrops).
    pub content: Option<Texture>,
    /// The sound (makepad-audio-reactive): looks read it with
    /// `self.audio_fft(f, age)`, `self.audio_wave(t)`, `self.audio_hit`, ...
    pub audio: Option<AudioFrame>,
}

/// Log bands of the animators' spectrum.
pub const BANDS: usize = makepad_audio_reactive::SPECTRUM_BANDS;

impl KineticFrame {
    /// The newest spectrum in [`BANDS`] log bands (0..1), which animators
    /// read as `band(f)`: the sound's, silence without one.
    pub fn spectrum(&self) -> &[f32] {
        const SILENCE: [f32; BANDS] = [0.0; BANDS];
        self.audio.as_ref().map_or(&SILENCE[..], |a| &a.bands[..])
    }
}

/// What the last frame cost on the CPU.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub elements: usize,
    pub draw_calls: usize,
    /// Records, kernel and bucketing (µs).
    pub animate_us: f32,
    pub kernel_us: f32,
    /// Recording the draw calls and passes (µs).
    pub record_us: f32,
}

/// The shader members a kit adds, as Splash text for its draw subclass.
fn members(split: &Split, values: &KitValues) -> String {
    let mut s = String::new();
    for (name, f) in &split.shader {
        s.push_str(&format!("    {name}: {}\n", f.text));
    }
    for (k, (name, _)) in values.dials.iter().enumerate().take(4) {
        let c = ["x", "y", "z", "w"][k];
        if !split.shader.iter().any(|(n, _)| n == name) {
            s.push_str(&format!("    {name}: fn() -> float {{ return self.p.{c} }}\n"));
        }
    }
    s
}

/// The kit's helper fns and dial accessors for the backdrop (every shader
/// fn but the glyph stage's own).
fn helpers(split: &Split, values: &KitValues) -> String {
    let mut s = String::new();
    for (name, f) in &split.shader {
        if !matches!(name.as_str(), "look" | "floor" | "deform") {
            s.push_str(&format!("    {name}: {}\n", f.text));
        }
    }
    for (k, (name, _)) in values.dials.iter().enumerate().take(4) {
        let c = ["x", "y", "z", "w"][k];
        if !split.shader.iter().any(|(n, _)| n == name) {
            s.push_str(&format!("    {name}: fn() -> float {{ return self.p.{c} }}\n"));
        }
    }
    s
}

/// Register the draw shaders and the kit module (after `makepad_draw::script_mod`
/// and `makepad_render_graph::script_mod_passes`).
pub fn script_mod(vm: &mut ScriptVm) {
    makepad_script_compute::module::register_shared_std(vm);
    // Once per VM, however many hosts ask.
    let draw = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str("draw").into(), NoTrap).as_object();
    let have = draw.is_some_and(|d| {
        let v = vm.bx.heap.value(d, LiveId::from_str("DrawKineticGlyph").into(), NoTrap);
        !v.is_nil() && !v.is_err()
    });
    if !have {
        makepad_render_graph::script_mod_passes(vm);
        crate::draw::script_mod(vm);
    }
    kit::script_mod(vm);
}

fn compile_shader<T: ScriptNew + ScriptApply>(cx: &mut Cx, base: &str, members: &str, file: &str) -> Result<T, String> {
    let _ = makepad_draw::makepad_platform::shader_error::take();
    let r = cx.with_vm(|vm| {
        let mut draw = T::script_new_with_default(vm);
        if members.trim().is_empty() {
            return Ok(draw);
        }
        let code = format!("use mod.std.*\nuse mod.pod.*\nuse mod.math.*\nuse mod.shader.*\nuse mod.draw\nuse mod.shared.*\nmod.draw.{base}{{\n{}{members}}}\n", makepad_audio_reactive::SPLASH);
        vm.bx.captured_errors = Some(Vec::new());
        // A body of its own the VM reclaims once the shader is built (the
        // shader cache keys a compile by its body and text).
        let value = vm.eval_transient(ScriptMod { file: file.to_string(), code, ..Default::default() });
        let errors = vm.take_errors();
        if value.is_err() || !errors.is_empty() {
            return Err(if errors.is_empty() { format!("{file}: the look did not evaluate") } else { errors.join("; ") });
        }
        draw.script_apply(vm, &Apply::Eval, &mut Scope::default(), value);
        Ok(draw)
    })?;
    match makepad_draw::makepad_platform::shader_error::take() {
        Some(e) => Err(format!("{file}: {e}")),
        None => Ok(r),
    }
}

/// The glyph draw's instance record as the kernel's `KineticGlyph` layout.
fn instance_layout(cx: &Cx, draw: &DrawKineticGlyph) -> Result<Layout, String> {
    let id = draw.draw_vars.draw_shader_id.ok_or("the glyph shader has no id")?;
    let l = cx.layout_of(id, LayoutKind::Instance, LayoutPacking::VertexFetch).map_err(|e| format!("layout_of: {e:?}"))?;
    let mut fields = Vec::new();
    for f in &l.fields {
        let ty = match f.ty {
            PodType::F32 => FieldTy::F32,
            PodType::I32 | PodType::U32 => FieldTy::I32,
            PodType::Vec2 => FieldTy::Vec2,
            PodType::Vec3 => FieldTy::Vec3,
            PodType::Vec4 => FieldTy::Vec4,
            PodType::Mat4 => FieldTy::Mat4,
            other => return Err(format!("instance field `{}` is a {other:?}", f.name)),
        };
        fields.push(LayoutField { name: f.name.to_string(), ty, offset: f.offset_words });
    }
    Ok(Layout { name: "KineticGlyph".into(), stride: l.stride_words, fields })
}

fn set_pass_camera(cx: &mut Cx, pass: &DrawPass, view: Mat4f, projection: Mat4f) {
    let camera_inv = view.invert();
    let gen = cx.next_uniform_gen();
    let p = &mut cx.passes[pass.draw_pass_id()];
    let u = &mut p.pass_uniforms;
    u.camera_projection = projection;
    u.camera_projection_r = projection;
    u.camera_view = view;
    u.camera_view_r = view;
    u.depth_projection = projection;
    u.depth_projection_r = projection;
    u.depth_view = view;
    u.depth_view_r = view;
    u.camera_inv = camera_inv;
    u.camera_inv_r = camera_inv;
    p.mark_pass_uniforms_dirty(gen);
}

/// A u x v grid over 0..1 (face class 6), shaped in the vertex stage by
/// the look's `surface(uv)`.
fn surface_shape(id: usize, u: u32, v: u32) -> shapes::Shape {
    let mut s = shapes::Shape { key: u32::MAX - 2, size: [1.0, 1.0, 0.0], ..Default::default() };
    for j in 0..=v {
        for i in 0..=u {
            let (a, b) = (i as f32 / u as f32, j as f32 / v as f32);
            s.verts.extend_from_slice(&[a, b, 0.0, id as f32, 0.0, 0.0, 1.0, 6.0, a, b, 1.0, 1.0]);
        }
    }
    for j in 0..v {
        for i in 0..u {
            let k = j * (u + 1) + i;
            s.indices.extend_from_slice(&[k, k + 1, k + u + 2, k, k + u + 2, k + u + 1]);
        }
    }
    s
}

/// A plane of size 1 in x and z, facing +y, as the floor shape.
fn floor_shape(id: usize) -> shapes::Shape {
    let mut s = shapes::Shape { key: u32::MAX - 1, size: [1.0, 0.0, 1.0], ..Default::default() };
    for (x, z) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
        s.verts.extend_from_slice(&[x, 0.0, z, id as f32, 0.0, 1.0, 0.0, 5.0, x + 0.5, z + 0.5, 1.0, 1.0]);
    }
    s.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    s
}

/// The picture pass of a kit with `picture`.
struct Picture {
    pass: DrawPass,
    list: DrawList,
    color: Texture,
    depth: Texture,
}

/// Bind `tex` to the texture slot the shader named `id` (no-op without it).
fn bind_named(cx: &Cx, dv: &mut DrawVars, id: LiveId, tex: &Texture) {
    if let Some(sid) = dv.draw_shader_id {
        if let Some(slot) = cx.draw_shaders[sid.index].mapping.textures.iter().position(|t| t.id == id) {
            dv.set_texture(slot, tex);
        }
    }
}

impl Picture {
    fn new(cx: &mut Cx) -> Self {
        let pass = DrawPass::new_with_name(cx, "kinetic picture");
        cx.passes[pass.draw_pass_id()].keep_camera_matrix = true;
        Self {
            pass,
            list: DrawList::new(cx),
            color: Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true }),
            depth: Texture::new_with_format(cx, TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true }),
        }
    }
}

/// What one frame draws: the records bucketed by shape, and the floor.
struct Glyphs<'a> {
    set: &'a GlyphSet,
    buckets: &'a [Vec<u32>],
    geometries: &'a [Geometry],
    out: &'a [f32],
    stride: usize,
    /// The floor's shape and its size in cap heights (None = auto).
    floor: Option<(usize, Option<f32>)>,
    surface: Option<(usize, u32)>,
    floor_y: f32,
    centre: [f32; 2],
    width: f32,
    height: f32,
    size: f32,
}

impl Glyphs<'_> {
    /// One instanced draw call per shape in use, then the floor; returns
    /// the draw calls.
    fn draw(&self, cx: &mut Cx2d, d: &mut DrawKineticGlyph) -> usize {
        let mut calls = 0;
        for (shape, bucket) in self.buckets.iter().enumerate() {
            if bucket.is_empty() || self.set.shapes[shape].indices.is_empty() {
                continue;
            }
            d.draw_vars.geometry_id = Some(self.geometries[shape].geometry_id());
            let Some(mut many) = cx.begin_many_instances(&d.draw_vars) else { continue };
            for &i in bucket {
                let i = i as usize;
                many.instances.extend_from_slice(&self.out[i * self.stride..(i + 1) * self.stride]);
            }
            let area = cx.end_many_instances(many);
            d.draw_vars.area = cx.update_area_refs(d.draw_vars.area, area);
            calls += 1;
        }
        if let Some((fid, fs)) = self.floor {
            let span = fs.map_or((self.width.max(self.height) * 3.0).max(self.size * 6.0), |s| s * self.size);
            d.pos = vec3f(self.centre[0], self.floor_y, 0.0);
            d.rot = vec4(0.0, 0.0, 0.0, 1.0);
            d.scale = vec3f(span, 1.0, span);
            d.shear = vec2f(0.0, 0.0);
            d.color = vec4(1.0, 1.0, 1.0, 1.0);
            // The floor's look reads its span and height here.
            d.attr = vec4(span, self.floor_y, 0.0, 0.0);
            d.info = vec4(0.0, 0.0, 0.0, -1.0);
            d.shape = fid as f32;
            d.draw_vars.geometry_id = Some(self.geometries[fid].geometry_id());
            if let Some(mut many) = cx.begin_many_instances(&d.draw_vars) {
                many.instances.extend_from_slice(d.draw_vars.as_slice());
                let area = cx.end_many_instances(many);
                d.draw_vars.area = cx.update_area_refs(d.draw_vars.area, area);
                calls += 1;
            }
        }
        calls
    }

    /// The surface's copies (drawn in the frame, reading the picture).
    fn draw_surface(&self, cx: &mut Cx2d, d: &mut DrawKineticGlyph) -> usize {
        let Some((sid, copies)) = self.surface else { return 0 };
        d.pos = vec3f(0.0, 0.0, 0.0);
        d.rot = vec4(0.0, 0.0, 0.0, 1.0);
        d.scale = vec3f(1.0, 1.0, 1.0);
        d.shear = vec2f(0.0, 0.0);
        d.color = vec4(1.0, 1.0, 1.0, 1.0);
        d.shape = sid as f32;
        d.draw_vars.geometry_id = Some(self.geometries[sid].geometry_id());
        let Some(mut many) = cx.begin_many_instances(&d.draw_vars) else { return 0 };
        for c in 0..copies {
            d.attr = vec4(c as f32, copies as f32, 0.0, 0.0);
            d.info = vec4(c as f32 / (copies.max(2) - 1) as f32, 0.0, 0.0, c as f32);
            many.instances.extend_from_slice(d.draw_vars.as_slice());
        }
        let area = cx.end_many_instances(many);
        d.draw_vars.area = cx.update_area_refs(d.draw_vars.area, area);
        1
    }
}

pub struct KineticView {
    pub split: Split,
    pub values: KitValues,
    /// The kit's own palette (a host override replaces `values.colors`).
    kit_colors: [Vec4f; 4],
    /// The kit's own font, for [`Self::set_font`]`(None)`.
    kit_font: crate::FontSource,
    /// Letters alone over a clear frame ([`KineticView::set_overlay`]).
    overlay: bool,
    draw: DrawKineticGlyph,
    backdrop: Option<DrawKineticBackdrop>,
    glyph_kernel: Arc<Kernel>,
    camera_kernel: Option<Arc<Kernel>>,
    curve_kernel: Option<Arc<Kernel>>,
    /// The curve's points, resampled data and texture.
    curve: (Vec<f32>, Vec<[f32; 3]>, Option<Texture>, f32),
    layout: Layout,
    shape_word: usize,
    set: Option<GlyphSet>,
    text: Option<String>,
    /// When the text last changed (host time).
    text_at: f32,
    geometries: Vec<Geometry>,
    floor: Option<usize>,
    /// The surface shape and its copies.
    surface: Option<(usize, u32)>,
    records: Records,
    out: Vec<f32>,
    buckets: Vec<Vec<u32>>,
    pass: DrawPass,
    list: DrawList,
    color: Texture,
    depth: Texture,
    picture: Option<Picture>,
    graph: GraphRunner,
    pub stats: FrameStats,
    pub errors: Vec<String>,
}

impl KineticView {
    /// Load a kit (its Splash text). `file` names it in diagnostics.
    pub fn new(cx: &mut Cx, source: &str, file: &str) -> Result<Self, String> {
        let split = kit::split(source)?;
        let values = cx.with_vm(|vm| kit::read_values(vm, &split, file))?;
        let draw: DrawKineticGlyph = compile_shader(cx, "DrawKineticGlyph", &members(&split, &values), file)?;
        let backdrop = match &split.backdrop {
            Some(f) => Some(compile_shader::<DrawKineticBackdrop>(cx, "DrawKineticBackdrop", &format!("    backdrop: {}\n{}", f.text, helpers(&split, &values)), file)?),
            None => None,
        };
        let layout = instance_layout(cx, &draw)?;
        let shape_word = layout.fields.iter().find(|f| f.name == "shape").map(|f| f.offset as usize).ok_or("the glyph record has no `shape`")?;
        let composed = kernel::compose(&split, &values.dials, "KineticGlyph");
        let glyph_kernel = kernel::compile(&composed, &layout)?;
        let camera_kernel = match &split.camera {
            Some(f) => Some(kernel::compile(&kernel::compose_camera(&split, &values.dials, f), &layout)?),
            None => None,
        };
        let curve_kernel = match (&split.curve, values.curve_points) {
            (Some(f), Some(_)) => Some(kernel::compile(&kernel::compose_curve(&split, &values.dials, f), &layout)?),
            (Some(_), None) => return Err("a kit with `curve_fn` also declares `curve: {points: 256}`".into()),
            _ => None,
        };
        let pass = DrawPass::new_with_name(cx, "kinetic");
        // The pass keeps the camera this view sets (not the 2D ortho).
        cx.passes[pass.draw_pass_id()].keep_camera_matrix = true;
        let color = Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
        let depth = Texture::new_with_format(cx, TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true });
        let mut graph = GraphRunner::default();
        graph.set_passes(&values.passes);
        Ok(Self {
            kit_colors: values.colors,
            kit_font: values.shape.font.clone(),
            overlay: false,
            split,
            values,
            draw,
            backdrop,
            glyph_kernel,
            camera_kernel,
            curve_kernel,
            curve: (Vec::new(), Vec::new(), None, 0.0),
            layout,
            shape_word,
            set: None,
            text: None,
            text_at: -1e9,
            geometries: Vec::new(),
            floor: None,
            surface: None,
            records: Records::default(),
            out: Vec::new(),
            buckets: Vec::new(),
            list: DrawList::new(cx),
            pass,
            color,
            depth,
            picture: None,
            graph,
            stats: FrameStats::default(),
            errors: Vec::new(),
        })
    }

    /// The text as the kit shows it (its case applied).
    pub fn shown(&self, text: &str) -> String {
        let t = if text.is_empty() { self.values.text.as_str() } else { text };
        if self.values.upper {
            t.to_uppercase()
        } else if self.values.lower {
            t.to_lowercase()
        } else {
            t.to_string()
        }
    }

    /// The shape settings for a text (to build a set off the UI thread).
    pub fn spec(&self, text: &str) -> shapes::ShapeSpec {
        shapes::ShapeSpec { text: self.shown(text), ..self.values.shape.clone() }
    }

    /// Show `text` (building its shapes now; a host that cannot wait
    /// builds `shapes::build(&view.spec(text))` on its pool and hands the
    /// result to [`Self::install`]). `now` stamps the glyphs that changed.
    pub fn set_text(&mut self, cx: &mut Cx, text: &str, now: f32) {
        let shown = self.shown(text);
        if self.text.as_deref() == Some(shown.as_str()) {
            return;
        }
        match shapes::build(&self.spec(text)) {
            Ok(set) => {
                self.install(cx, set, now);
                self.text = Some(shown);
            }
            Err(e) => self.note(e),
        }
    }

    /// Install a built set.
    pub fn install(&mut self, cx: &mut Cx, mut set: GlyphSet, now: f32) {
        let chars = set.elements.iter().map(|e| e.char_index).filter(|c| *c != usize::MAX).max().map_or(0, |m| m + 1);
        self.records.set(&set, self.values.copies, now, chars, self.values.dying.is_some());
        self.text_at = now;
        self.surface = None;
        if let Some((u, v, copies)) = self.values.surface {
            let id = set.shapes.len();
            set.shapes.push(surface_shape(id, u, v));
            self.surface = Some((id, copies));
        }
        self.floor = None;
        if self.values.floor.is_some() {
            let id = set.shapes.len();
            set.shapes.push(floor_shape(id));
            self.floor = Some(id);
        }
        while self.geometries.len() < set.shapes.len() {
            self.geometries.push(Geometry::new(cx));
        }
        for (g, s) in self.geometries.iter().zip(set.shapes.iter()) {
            if !s.indices.is_empty() {
                g.update(cx, s.indices.clone(), s.verts.clone());
            }
        }
        debug_assert!(set.shapes.iter().all(|s| s.verts.len() % VERT_FLOATS == 0));
        self.set = Some(set);
    }

    /// Read the font at other variable-axis values (`wdth`, `slnt`, any
    /// tag; `wght` is the weight): the text is rebuilt on the next
    /// `set_text` when they differ from the current ones.
    /// A host's palette (bg, a, b, c) over the kit's own (`None`: the
    /// kit's): a VJ console's colour override, a game's team colours.
    pub fn set_colors(&mut self, colors: Option<[Vec4f; 4]>) {
        self.values.colors = colors.unwrap_or(self.kit_colors);
    }

    /// OVERLAY: the letters alone over a clear frame, no backdrop (a game's
    /// banner over play, a title over footage); off, the kit's whole frame.
    pub fn set_overlay(&mut self, overlay: bool) {
        self.overlay = overlay;
    }

    /// A host's font in place of the kit's own (`None`: the kit's): a
    /// document's font for a kinetic title. The text is rebuilt on the next
    /// `set_text` when it changes.
    pub fn set_font(&mut self, font: Option<crate::FontSource>) {
        let font = font.unwrap_or_else(|| self.kit_font.clone());
        if font != self.values.shape.font {
            self.values.shape.font = font;
            self.text = None;
        }
    }

    pub fn set_axes(&mut self, weight: Option<f32>, axes: &[(u32, f32)]) {
        let weight = weight.or(self.values.shape.weight);
        let mut merged = self.values.shape.axes.clone();
        for (tag, v) in axes {
            match merged.iter_mut().find(|(t, _)| t == tag) {
                Some(a) => a.1 = *v,
                None => merged.push((*tag, *v)),
            }
        }
        if weight != self.values.shape.weight || merged != self.values.shape.axes {
            self.values.shape.weight = weight;
            self.values.shape.axes = merged;
            self.text = None;
        }
    }

    /// The pass this view renders its scene in (the first of its passes;
    /// the kit's post passes follow it).
    pub fn scene_pass(&self) -> DrawPassId {
        self.pass.draw_pass_id()
    }

    fn bind_picture(cx: &Cx, dv: &mut DrawVars, pp: &Picture, picture: Option<(u32, u32, f32)>) {
        bind_named(cx, dv, live_id!(pic_tex), &pp.color);
        let (w, h) = picture.map_or((1024.0, 256.0), |p| (p.0 as f32, p.1 as f32));
        dv.set_uniform(cx, live_id!(k_pic), &[w, h, 0.0, 0.0]);
    }

    pub fn has_text(&self) -> bool {
        self.set.is_some()
    }

    fn note(&mut self, e: String) {
        if !self.errors.contains(&e) {
            log!("kinetic: {e}");
            self.errors.push(e);
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn set_uniforms<D: std::ops::DerefMut<Target = DrawVars>>(values: &KitValues, dv: &mut D, cx: &Cx, s: &[f32; 7], p: [f32; 4], bands: [f32; 4], misc: [f32; 4], view: [f32; 4], text: [f32; 4], share: [f32; 4]) {
        dv.set_uniform(cx, live_id!(k_share), &share);
        for (k, name) in ["time", "beat", "phase", "pulse", "bar", "energy", "bpm"].iter().enumerate() {
            dv.set_uniform(cx, LiveId::from_str(name), &[s[k]]);
        }
        dv.set_uniform(cx, live_id!(p), &p);
        dv.set_uniform(cx, live_id!(bands), &bands);
        let c = &values.colors;
        dv.set_uniform(cx, live_id!(col_bg), &[c[0].x, c[0].y, c[0].z, c[0].w]);
        dv.set_uniform(cx, live_id!(col_a), &[c[1].x, c[1].y, c[1].z, c[1].w]);
        dv.set_uniform(cx, live_id!(col_b), &[c[2].x, c[2].y, c[2].z, c[2].w]);
        dv.set_uniform(cx, live_id!(col_c), &[c[3].x, c[3].y, c[3].z, c[3].w]);
        dv.set_uniform(cx, live_id!(k_misc), &misc);
        dv.set_uniform(cx, live_id!(k_text), &text);
        dv.set_uniform(cx, live_id!(k_view), &view);
    }

    /// Animate and draw one frame into the target (`px` pixels); returns
    /// the picture (after the kit's passes).
    pub fn render(&mut self, cx: &mut Cx2d, px: (u32, u32), frame: &KineticFrame) -> Option<Texture> {
        let set = self.set.as_ref()?;
        let t0 = Cx::monotonic_now();
        // ---- signals
        let bpm = if frame.bpm > 0.0 { frame.bpm } else { 120.0 };
        let beat = frame.beat;
        let phase = beat - beat.floor();
        let pulse = (-phase * 5.0).exp();
        let bar = (beat / 4.0).fract();
        let grow = if self.values.cycle_beats > 0.0 {
            let u = (beat / self.values.cycle_beats).fract();
            if self.values.pingpong { 1.0 - (u * 2.0 - 1.0).abs() } else { u }
        } else {
            1.0
        };
        let (bmin, bmax) = set.bounds;
        let (width, height) = ((bmax[0] - bmin[0]).max(0.001), (bmax[1] - bmin[1]).max(0.001));
        let size = self.values.shape.size;
        let mut p = [0.5f32; 4];
        for k in 0..4 {
            if let Some((_, d)) = self.values.dials.get(k) {
                p[k] = *d;
            }
            if let Some(v) = frame.dials[k] {
                p[k] = v;
            }
        }
        // ---- records and the animator
        if self.records.dying > 0 && self.values.dying.is_none_or(|d| frame.time - self.text_at > d) {
            self.records.retire_dying();
        }
        self.records.sing(&frame.karaoke);
        let n = self.records.count;
        let stride = self.layout.stride as usize;
        self.out.resize(n * stride, 0.0);
        let (view_w, view_h) = match self.values.picture {
            Some((pw, ph, v)) => (v * size * pw as f32 / ph.max(1) as f32, v * size),
            None => (0.0, 0.0),
        };
        let floor_y = self.values.floor.map_or(bmin[1] - 0.05 * size, |(y, _)| y.map_or(bmin[1] - 0.05 * size, |y| y * size));
        let sig = [
            beat,
            phase,
            pulse,
            bar,
            bpm,
            frame.energy,
            grow,
            width,
            height,
            size,
            set.alpha0 as f32,
            set.alphas as f32,
            set.cube.map_or(0.0, |c| c as f32),
            self.text_at,
            frame.bands[0],
            frame.bands[1],
            frame.bands[2],
            floor_y,
            view_w,
            view_h,
        ];
        let tk = Cx::monotonic_now();
        {
            let mut call = self.glyph_kernel.call();
            call.set_time(frame.time);
            for ((name, _), v) in SIGNALS.iter().zip(sig.iter()) {
                call.set_param(name, *v);
            }
            for k in 0..4 {
                call.set_param(&format!("p{}", k + 1), p[k]);
                if let Some((name, _)) = self.values.dials.get(k) {
                    call.set_param(name, p[k]);
                }
            }
            let ok = call.input("glyphs", &self.records.data[..n * GLYPH_WORDS]).and_then(|_| call.input("spectrum", frame.spectrum())).and_then(|_| call.output("out", &mut self.out[..]));
            let run = ok.and_then(|_| if n > 2048 { call.run_parallel(n, 8) } else { call.run(n) });
            if let Err(e) = run {
                let e = format!("animator: {e}");
                if !self.errors.contains(&e) {
                    log!("kinetic: {e}");
                    self.errors.push(e);
                }
            }
        }
        let kernel_us = ((Cx::monotonic_now() - tk) * 1e6) as f32;
        // ---- bucket by shape
        let shapes = set.shapes.len();
        self.buckets.resize_with(shapes.max(self.buckets.len()), Vec::new);
        for b in &mut self.buckets {
            b.clear();
        }
        for i in 0..n {
            let s = self.out[i * stride + self.shape_word];
            if s.is_finite() && s >= 0.0 && (s as usize) < shapes {
                self.buckets[s.round() as usize].push(i as u32);
            }
        }
        // ---- camera
        let aspect = px.0 as f32 / px.1.max(1) as f32;
        let fov = self.values.fov;
        let tan = (fov.to_radians() * 0.5).tan();
        // Frame the extents (the height at the vertical fov, the width at
        // the frame's aspect, a margin on both), level with the text, with
        // a slow drift so a still text still breathes.
        let dist = match self.values.dist {
            Some(d) => d * size,
            None => ((height * 0.5 * 1.25 + size * 0.3).max(width * 0.5 * 1.15 / aspect) / tan.max(0.05)).max(size * 2.0) + bmax[2].max(0.0),
        };
        let centre = [(bmin[0] + bmax[0]) * 0.5, (bmin[1] + bmax[1]) * 0.5];
        let drift = [(frame.time * 0.19).sin() * width * 0.02, (frame.time * 0.13).sin() * height * 0.02];
        let lift = match self.values.height {
            Some(h) => h * size,
            None if self.values.floor.is_some() => height * 0.35 + size * 0.8,
            None => 0.0,
        };
        let mut cam = [centre[0] + drift[0], centre[1] + lift + drift[1], dist, centre[0], centre[1], 0.0, 0.0, 1.0, 0.0, fov, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        if let Some(k) = &self.camera_kernel {
            let base = cam;
            let mut o = [0.0f32; 16];
            let mut call = k.call();
            call.set_time(frame.time);
            for ((name, _), v) in SIGNALS.iter().zip(sig.iter()) {
                call.set_param(name, *v);
            }
            for k in 0..4 {
                call.set_param(&format!("p{}", k + 1), p[k]);
            }
            let r = call.input("base", &base).and_then(|_| call.input("spectrum", frame.spectrum())).and_then(|_| call.output("out", &mut o)).and_then(|_| call.run(1));
            match r {
                Ok(_) => cam = o,
                Err(e) => {
                    let e = format!("camera: {e}");
                    if !self.errors.contains(&e) {
                        self.errors.push(e);
                    }
                }
            }
        }
        if let (Some(k), Some(np)) = (&self.curve_kernel, self.values.curve_points) {
            let np = np as usize;
            self.curve.0.resize(np * 4, 0.0);
            let mut call = k.call();
            call.set_time(frame.time);
            for ((name, _), v) in SIGNALS.iter().zip(sig.iter()) {
                call.set_param(name, *v);
            }
            for k in 0..4 {
                call.set_param(&format!("p{}", k + 1), p[k]);
            }
            let r = call.input("spectrum", frame.spectrum()).and_then(|_| call.output("out", &mut self.curve.0)).and_then(|_| call.run(np));
            match r {
                Ok(_) => {
                    self.curve.1.clear();
                    self.curve.1.extend(self.curve.0.chunks_exact(4).map(|c| [c[0], c[1], c[2]]));
                    let (data, len) = crate::curve::resample(&self.curve.1, np, self.values.curve_frames);
                    self.curve.3 = len;
                    match &self.curve.2 {
                        Some(t) => {
                            let _ = t.take_vec_f32(cx.cx);
                            t.put_back_vec_f32(cx.cx, data, None);
                        }
                        None => {
                            self.curve.2 = Some(Texture::new_with_format(cx.cx, TextureFormat::VecRGBAf32 { width: np, height: crate::curve::ROWS, data: Some(data), updated: TextureUpdated::Full }));
                        }
                    }
                }
                Err(e) => {
                    let e = format!("curve: {e}");
                    if !self.errors.contains(&e) {
                        self.errors.push(e);
                    }
                }
            }
        }
        let share = [cam[12], cam[13], cam[14], cam[15]];
        let view = Mat4f::look_at(vec3f(cam[0], cam[1], cam[2]), vec3f(cam[3], cam[4], cam[5]), vec3f(cam[6], cam[7], cam[8]));
        let near = (dist * 0.02).max(0.01);
        let projection = Mat4f::perspective(cam[9].clamp(1.0, 170.0), aspect, near, near * 5000.0);
        self.stats.animate_us = ((Cx::monotonic_now() - t0) * 1e6) as f32;
        self.stats.kernel_us = kernel_us;
        self.stats.elements = n;
        // ---- record
        let tr = Cx::monotonic_now();
        let size_px = dvec2(px.0 as f64, px.1 as f64);
        let bg = self.values.colors[0];
        let s = [frame.time, beat, phase, pulse, bar, frame.energy, bpm];
        let misc = [self.values.material, if frame.content.is_some() { 1.0 } else { 0.0 }, width, height];
        let picture = self.values.picture;
        let viewu = [px.0 as f32, px.1 as f32, self.text_at, if picture.is_some() { 1.0 } else { 0.0 }];
        let bands = [frame.bands[0], frame.bands[1], frame.bands[2], frame.energy];
        let textu = [size, set.lines as f32, n as f32, set.words as f32];
        let mut calls = 0;
        let glyphs = Glyphs { set, buckets: &self.buckets, geometries: &self.geometries, out: &self.out, stride, floor: self.floor.zip(self.values.floor.map(|f| f.1)), surface: None, floor_y, centre, width, height, size };
        // With a picture the glyphs draw flat into it (an orthographic
        // view `view` cap heights tall about the origin), and the frame is
        // the backdrop reading it.
        if let Some((pw, ph, _)) = picture {
            let pp = self.picture.get_or_insert_with(|| Picture::new(cx.cx));
            let psize = dvec2(pw as f64, ph as f64);
            pp.pass.set_size(cx.cx, psize);
            pp.pass.set_color_texture(cx.cx, &pp.color, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
            pp.pass.set_depth_texture(cx.cx, &pp.depth, DrawPassClearDepth::ClearWith(1.0));
            cx.make_child_pass(&pp.pass);
            cx.begin_pass(&pp.pass, Some(1.0));
            pp.pass.set_size(cx.cx, psize);
            let view = Mat4f::look_at(vec3f(0.0, 0.0, 100.0), vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0));
            // Orthographic: the view's width and height to clip space, view
            // depth 1..1000 to 0..1.
            let (near, far) = (1.0f32, 1000.0f32);
            let mut proj = Mat4f::identity();
            proj.v[0] = 2.0 / view_w.max(1e-6);
            proj.v[5] = 2.0 / view_h.max(1e-6);
            proj.v[10] = -1.0 / (far - near);
            proj.v[14] = -near / (far - near);
            set_pass_camera(cx.cx, &pp.pass, view, proj);
            pp.list.begin_always(cx);
            let pview = [pw as f32, ph as f32, self.text_at, 1.0];
            Self::set_uniforms(&self.values, &mut self.draw, cx.cx, &s, p, bands, misc, pview, textu, share);
            if let Some(a) = &frame.audio {
                bind_audio(cx.cx, &mut self.draw.draw_vars, a);
            }
            calls += glyphs.draw(cx, &mut self.draw);
            pp.list.end(cx);
            cx.end_pass(&pp.pass);
        }
        self.pass.set_size(cx.cx, size_px);
        let clear = if self.overlay { vec4(0.0, 0.0, 0.0, 0.0) } else { vec4(bg.x, bg.y, bg.z, 1.0) };
        self.pass.set_color_texture(cx.cx, &self.color, DrawPassClearColor::ClearWith(clear));
        self.pass.set_depth_texture(cx.cx, &self.depth, DrawPassClearDepth::ClearWith(1.0));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(1.0));
        self.pass.set_size(cx.cx, size_px);
        set_pass_camera(cx.cx, &self.pass, view, projection);
        self.list.begin_always(cx);
        if let Some(b) = self.backdrop.as_mut().filter(|_| !self.overlay) {
            Self::set_uniforms(&self.values, &mut b.draw_super, cx.cx, &s, p, bands, misc, viewu, textu, share);
            if let Some(c) = &frame.content {
                b.draw_super.draw_vars.set_texture(0, c);
            }
            if let Some(a) = &frame.audio {
                bind_audio(cx.cx, &mut b.draw_super.draw_vars, a);
            }
            if let Some(pp) = &self.picture {
                Self::bind_picture(cx.cx, &mut b.draw_super.draw_vars, pp, picture);
            }
            b.draw_super.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: size_px });
            calls += 1;
        }
        Self::set_uniforms(&self.values, &mut self.draw, cx.cx, &s, p, bands, misc, viewu, textu, share);
        if let Some(c) = &frame.content {
            self.draw.draw_vars.set_texture(0, c);
        }
        if let Some(a) = &frame.audio {
            bind_audio(cx.cx, &mut self.draw.draw_vars, a);
        }
        if let Some(pp) = &self.picture {
            Self::bind_picture(cx.cx, &mut self.draw.draw_vars, pp, picture);
        }
        if let Some(t) = &self.curve.2 {
            bind_named(cx.cx, &mut self.draw.draw_vars, live_id!(curve_tex), t);
            let np = self.values.curve_points.unwrap_or(2) as f32;
            self.draw.draw_vars.set_uniform(cx.cx, live_id!(k_curve), &[self.curve.3, np, 0.0, 0.0]);
        }
        if picture.is_none() {
            calls += glyphs.draw(cx, &mut self.draw);
        }
        calls += Glyphs { surface: self.surface, ..glyphs }.draw_surface(cx, &mut self.draw);
        self.list.end(cx);
        cx.end_pass(&self.pass);
        if let (Some(pp), Some(_)) = (&self.picture, picture) {
            cx.cx.passes[pp.pass.draw_pass_id()].parent = CxDrawPassParent::DrawPass(self.pass.draw_pass_id());
        }
        // ---- the kit's passes
        let mut out = self.color.clone();
        if !self.graph.is_empty() && self.graph.prepare(cx.cx, px, Attachments::default(), &[], false, 1 << 30) {
            let values = self.values.pass_values.clone();
            let fu = FrameUniforms { time: frame.time, frame: 0.0, ss_tap: -1.0, seed: 0.0, exposure: 1.0, camera: [-1.0, -0.2, 0.0, 1.0] };
            let mut producer = self.pass.draw_pass_id();
            for stage in Stage::ALL {
                let inputs = StageInputs { color: &out, depth: None, normal: None, velocity: None, glow: None, id: None, named: &[] };
                if let Some((tex, first)) = self.graph.run_stage(cx, stage, &inputs, &values, fu, None) {
                    cx.cx.passes[producer].parent = CxDrawPassParent::DrawPass(first);
                    if let Some(last) = self.graph.pass_ids(stage).last() {
                        producer = *last;
                    }
                    out = tex;
                }
            }
        }
        for e in std::mem::take(&mut self.graph.errors) {
            self.note(e);
        }
        self.stats.draw_calls = calls;
        self.stats.record_us = ((Cx::monotonic_now() - tr) * 1e6) as f32;
        Some(out)
    }
}
