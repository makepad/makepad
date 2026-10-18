//! The skyline core: one view of the 360 degree night band, drawn as a chain of passes.
//!
//! ```text
//!   producers        trees (coverage, one channel per tree layer)    stage B
//!                    city  (march: colour+alpha, emission+depth)      stage A
//!   scene pass       two RGBA16F targets, every instance premultiplied-over in painter's order:
//!                      glow, [props before range k, range k] k = 0..4, props behind the city,
//!                      the city, props in front of it, the front treeline, the ground band,
//!                      the props again over the sea band
//!   mirror pass      the same list seen mirrored (water reflections), when post asks for it
//!   post             the blur of the lights, then the final composite with the tone shoulder  stage D
//! ```
//!
//! The CPU inputs (zones, columns) are cached by the parameters they depend on, and the passes
//! are only recorded again when the picture changes: a repaint of the window just draws the final
//! quad over the retained targets. The core depends on `makepad_draw` only.
use crate::city::City;
use crate::params::{Params, HAZE_COL, P, SIL};
use crate::post::Post;
use crate::props::{Bucket, Props};
use crate::rng::TAU;
use crate::shaders::*;
use crate::terrain::{self, Trees, ROWS, TEX_W};
use crate::zones::{mt_scale, Zones};
use makepad_widgets::makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.SkylineBase = #(Skyline::script_component(vm))
    mod.draw.Skyline = mod.std.set_type_default() do mod.draw.SkylineBase{
        draw_glow: mod.draw.DrawSkyGlow{}
        draw_range: mod.draw.DrawSkyRange{}
        draw_ground: mod.draw.DrawSkyGround{}
        trees: mod.draw.SkyTrees{}
        city: mod.draw.SkyCity{}
        props: mod.draw.SkyProps{}
        post: mod.draw.SkyPost{}
    }
}

/// What a view shows and at what size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewSpec {
    /// The first azimuth (0..1 = 0..360 degrees) and the share of the turn shown.
    pub u0: f64,
    pub span: f64,
    /// Size in pixels.
    pub w: usize,
    pub h: usize,
    /// The horizon line, a fraction of the height from the top.
    pub horizon: f64,
}

impl ViewSpec {
    /// The full turn, `w` pixels wide.
    pub fn strip(p: &Params, w: usize) -> Self {
        Self::new(p, 0.0, 1.0, w)
    }
    /// The detail view: `zoom` of the turn centred on `pan`.
    pub fn detail(p: &Params, w: usize) -> Self {
        Self::new(p, p[P::Pan] - p[P::Zoom] / 2.0, p[P::Zoom], w)
    }
    pub fn new(p: &Params, u0: f64, span: f64, w: usize) -> Self {
        let w = w.max(8);
        let h = ((w as f64 * p[P::Vfov] / (span * 360.0)).round() as usize).max(8);
        Self { u0, span, w, h, horizon: p[P::Horizon] }
    }
    /// Radians per pixel of the raw view.
    pub fn px0(&self) -> f64 {
        self.span * TAU / self.w as f64
    }
    /// Pass row of raw elevation `ang` (rad).
    pub fn row_of(&self, ang: f64) -> f64 {
        self.horizon * self.h as f64 - ang / self.px0()
    }
}

/// One scene target: a pass with two RGBA16F targets (colour + coverage, light + coverage).
pub struct SceneTarget {
    pub pass: DrawPass,
    pub list: DrawList2d,
    pub color: Texture,
    pub light: Texture,
}

impl SceneTarget {
    pub fn new(cx: &mut Cx, name: &str) -> Self {
        let pass = DrawPass::new_with_name(cx, name);
        let target = |cx: &mut Cx| {
            let t = Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
            pass.add_color_texture(cx, &t, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
            t
        };
        let (color, light) = (target(cx), target(cx));
        Self { list: DrawList2d::new(cx), pass, color, light }
    }

    /// Records the pass at `size`; `body` draws into it.
    pub fn record(&mut self, cx: &mut Cx2d, w: usize, h: usize, body: impl FnOnce(&mut Cx2d)) {
        let size = dvec2(w as f64, h as f64);
        self.pass.set_size(cx, size);
        cx.begin_pass(&self.pass, Some(1.0));
        self.list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        body(cx);
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }
}

/// What every scene drawer gets: the view, the parameters, the zones and the shared inputs.
pub struct SceneCtx<'a> {
    pub view: &'a ViewSpec,
    pub p: &'a Params,
    /// The placement zones (the city and the props read them).
    #[allow(dead_code)]
    pub z: &'a Zones,
    /// The column data texture and its geometry (width, height, margin, count).
    pub cols: &'a Texture,
    pub col_geom: [f32; 4],
    /// The column data on the CPU (see [`terrain::ROWS`]).
    pub col_data: &'a [[f32; 4]],
    /// The single trees' coverage (or a blank texture).
    pub tree_cov: Texture,
    /// The city march's targets once stage A provides them: colour + alpha, emission + depth (m).
    pub city: Option<(Texture, Texture)>,
    /// The reflection run: everything seen mirrored about the horizon (or its own waterline).
    pub mirror: bool,
}

impl SceneCtx<'_> {
    /// Every parameter (by its key), the view and the shared textures onto a scene shader.
    pub fn set_uniforms(&self, cx: &Cx, vars: &mut DrawVars) {
        self.p.set_uniforms(cx, vars);
        let v = self.view;
        vars.set_uniform(cx, live_id!(view_size), &[v.w as f32, v.h as f32]);
        vars.set_uniform(cx, live_id!(view_span), &[v.u0 as f32, v.span as f32]);
        vars.set_uniform(cx, live_id!(view_horizon), &[v.horizon as f32]);
        vars.set_uniform(cx, live_id!(col_geom), &self.col_geom);
        vars.set_uniform(cx, live_id!(mirror), &[if self.mirror { 1.0 } else { 0.0 }]);
        vars.set_uniform(cx, live_id!(sil), &SIL);
        vars.set_uniform(cx, live_id!(haze_col), &HAZE_COL);
        bind_texture(cx, vars, live_id!(cols), self.cols);
        bind_texture(cx, vars, live_id!(tree_cov), &self.tree_cov);
    }

    /// The column data of output column x (0..w) and row r.
    pub fn col(&self, x: usize, r: usize) -> [f32; 4] {
        self.col_data[(x + self.col_geom[2] as usize) * ROWS + r]
    }

    /// The pass rect from raw elevation `top` down to `bottom` (rad), across the view, flipped in
    /// the reflection run; None when it is empty.
    pub fn band(&self, top: f64, bottom: f64) -> Option<Rect> {
        let v = self.view;
        let (a, b) = if self.mirror { (-bottom, -top) } else { (top, bottom) };
        let y0 = v.row_of(a).floor().max(0.0);
        let y1 = v.row_of(b).ceil().min(v.h as f64);
        (y1 > y0).then(|| Rect { pos: dvec2(0.0, y0), size: dvec2(v.w as f64, y1 - y0) })
    }
}

/// Milliseconds since `t0` (seconds of the platform's monotonic clock).
fn ms_since(t0: f64) -> f64 {
    (Cx::monotonic_now() - t0) * 1000.0
}

/// Binds `texture` to the shader's texture input `name`, if it has one.
pub fn bind_texture(cx: &Cx, vars: &mut DrawVars, name: LiveId, texture: &Texture) {
    if let Some(id) = vars.draw_shader_id {
        if let Some(slot) = cx.draw_shaders[id.index].mapping.textures.iter().position(|t| t.id == name) {
            vars.set_texture(slot, texture);
        }
    }
}

/// What the last draws of a view cost.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Size of the last view drawn.
    pub w: usize,
    pub h: usize,
    /// CPU: zones, columns (when rebuilt) and recording the passes, ms.
    pub zones_ms: f64,
    pub cols_ms: f64,
    pub record_ms: f64,
    /// How many times the passes were recorded.
    pub renders: u64,
}

struct ColumnCache {
    key: u64,
    m: usize,
    data: Vec<[f32; 4]>,
    tex: Texture,
    geom: [f32; 4],
}

/// The parameters the zones depend on (with the seed).
const ZONE_KEYS: &[P] = &[
    P::CtC, P::CtW, P::CtR, P::Land, P::LandC, P::PrInd, P::PrTown, P::PrBridge, P::PrFair, P::PrTurb, P::PrMast, P::MtH, P::MtW, P::MtC,
    P::MtRug, P::TrC, P::TrW, P::TrH,
];

#[derive(Script, ScriptHook)]
pub struct Skyline {
    #[live]
    draw_glow: DrawSkyGlow,
    #[live]
    draw_range: DrawSkyRange,
    #[live]
    draw_ground: DrawSkyGround,
    #[live]
    pub trees: Trees,
    #[live]
    pub city: City,
    #[live]
    pub props: Props,
    #[live]
    pub post: Post,
    #[rust]
    scene: Option<SceneTarget>,
    #[rust]
    mirror: Option<SceneTarget>,
    #[rust]
    zones: Option<(u64, Zones)>,
    #[rust]
    cols: Option<ColumnCache>,
    #[rust]
    blank: Option<Texture>,
    /// The key of the picture the targets hold.
    #[rust]
    drawn: Option<u64>,
    #[rust]
    pub stats: Stats,
    /// A running export or band (`export.rs`), on a Skyline of its own.
    #[rust]
    pub job: Option<crate::export::Job>,
}

impl Skyline {
    /// Draws `view` into `rect` of the current pass: records the passes when the picture changed,
    /// then the final composite (`tone`: the display tone shoulder; an export keeps the raw HDR).
    pub fn draw_view(&mut self, cx: &mut Cx2d, rect: Rect, view: &ViewSpec, p: &Params, tone: bool) {
        self.update_inputs(cx, view, p);
        // the picture: every parameter but the view's own (pan, zoom and the field of view only
        // place the view, which its first azimuth, span and size say), and the view
        let view_only = [P::Pan as usize, P::Zoom as usize, P::Vfov as usize];
        let mut picture: Vec<f64> = p.v.iter().enumerate().filter(|(i, _)| !view_only.contains(i)).map(|(_, x)| *x).collect();
        picture.extend([view.u0, view.span, view.w as f64, view.h as f64, view.horizon]);
        let key = p.hash(&[], &picture);
        if self.scene.is_none() {
            self.scene = Some(SceneTarget::new(cx, "skyline_scene"));
        }
        if self.drawn != Some(key) {
            let t1 = Cx::monotonic_now();
            self.record(cx, view, p);
            self.drawn = Some(key);
            self.stats.renders += 1;
            self.stats.record_ms = ms_since(t1);
        }
        let (scene, mirror) = (self.scene.as_ref().unwrap(), self.mirror.as_ref());
        self.post.draw_final(cx, rect, p, &crate::post::SceneOut { view, scene, mirror }, tone);
        self.stats.w = view.w;
        self.stats.h = view.h;
    }

    /// The zones and the columns for this view, rebuilt when what they depend on changed.
    fn update_inputs(&mut self, cx: &mut Cx, view: &ViewSpec, p: &Params) {
        let zkey = p.hash(ZONE_KEYS, &[]);
        if self.zones.as_ref().map(|z| z.0) != Some(zkey) {
            let t = Cx::monotonic_now();
            self.zones = Some((zkey, Zones::new(p)));
            self.stats.zones_ms = ms_since(t);
        }
        let z = &self.zones.as_ref().unwrap().1;
        let m = terrain::margin(p, view.span, view.w);
        let ckey = p.hash(terrain::COLUMN_KEYS, &[view.u0, view.span, view.w as f64, m as f64]);
        if self.cols.as_ref().map(|c| c.key) != Some(ckey) {
            let t = Cx::monotonic_now();
            let data = terrain::columns(z, p, view.u0, view.span, view.w, m);
            let n = view.w + 2 * m;
            let tw = n.min(TEX_W);
            let bands = n.div_ceil(tw);
            let mut texels = vec![0f32; tw * ROWS * bands * 4];
            for (x, col) in data.chunks(ROWS).enumerate() {
                let (b, xx) = (x / tw, x % tw);
                for (r, v) in col.iter().enumerate() {
                    let o = ((r + ROWS * b) * tw + xx) * 4;
                    texels[o..o + 4].copy_from_slice(v);
                }
            }
            let format = TextureFormat::VecRGBAf32 { width: tw, height: ROWS * bands, data: Some(texels), updated: TextureUpdated::Full };
            let tex = match self.cols.take() {
                Some(c) => {
                    *c.tex.get_format(cx) = format;
                    c.tex
                }
                None => Texture::new_with_format(cx, format),
            };
            let geom = [tw as f32, (ROWS * bands) as f32, m as f32, n as f32];
            self.cols = Some(ColumnCache { key: ckey, m, data, tex, geom });
            self.stats.cols_ms = ms_since(t);
        }
        if self.blank.is_none() {
            self.blank = Some(Texture::new_with_format(cx, TextureFormat::VecRGBAf32 {
                width: 1,
                height: 1,
                data: Some(vec![0.0; 4]),
                updated: TextureUpdated::Full,
            }));
        }
    }

    /// Records every pass of the view: the producers, the scene, the mirror, the post passes.
    fn record(&mut self, cx: &mut Cx2d, view: &ViewSpec, p: &Params) {
        let z = &self.zones.as_ref().unwrap().1;
        let cols = self.cols.as_ref().unwrap();
        debug_assert_eq!(cols.m, cols.geom[2] as usize);
        let blank = self.blank.as_ref().unwrap();
        let mut scene = self.scene.take().unwrap();
        let scene_id = scene.pass.draw_pass_id();
        let mut sc = SceneCtx {
            view,
            p,
            z,
            cols: &cols.tex,
            col_geom: cols.geom,
            col_data: &cols.data,
            tree_cov: blank.clone(),
            city: None,
            mirror: false,
        };
        // producers: children of the scene pass, which samples them
        self.trees.draw_pass(cx, &sc, scene_id);
        self.city.draw_pass(cx, &sc, scene_id);
        sc.tree_cov = self.trees.coverage().unwrap_or(blank).clone();
        sc.city = self.city.textures();
        self.props.update(z, p);
        // the scene, and its mirror image when the water reflects it
        let parts = ScenePainter { glow: &mut self.draw_glow, range: &mut self.draw_range, ground: &mut self.draw_ground };
        let mut parts = (parts, &mut self.props, &mut self.city);
        scene.record(cx, view.w, view.h, |cx| draw_scene(cx, &sc, &mut parts));
        if self.post.wants_mirror(p, z) {
            let mut mirror = self.mirror.take().unwrap_or_else(|| SceneTarget::new(cx, "skyline_mirror"));
            sc.mirror = true;
            mirror.record(cx, view.w, view.h, |cx| draw_scene(cx, &sc, &mut parts));
            sc.mirror = false;
            self.mirror = Some(mirror);
        } else {
            self.mirror = None;
        }
        self.post.draw_passes(cx, &sc, &crate::post::SceneOut { view, scene: &scene, mirror: self.mirror.as_ref() });
        self.scene = Some(scene);
    }
}

/// The foundation's own scene shaders.
struct ScenePainter<'a> {
    glow: &'a mut DrawSkyGlow,
    range: &'a mut DrawSkyRange,
    ground: &'a mut DrawSkyGround,
}

/// The scene list in painter's order (far to near). The mirror run leaves out what stands below
/// the eye's own horizon: the front treeline and the ground band.
fn draw_scene(cx: &mut Cx2d, sc: &SceneCtx, (sp, props, city): &mut (ScenePainter, &mut Props, &mut City)) {
    let (v, p) = (sc.view, sc.p);
    let px0 = v.px0();
    // the glow dome over downtown, behind everything
    let c_d = 28.0 * p[P::CtR];
    let (dx, dy) = (
        (p[P::CtC] * TAU).cos() * c_d - 0.8 * p[P::Off] * c_d * (p[P::OffDir] * TAU).cos(),
        (p[P::CtC] * TAU).sin() * c_d - 0.8 * p[P::Off] * c_d * (p[P::OffDir] * TAU).sin(),
    );
    let dome_k = (dx * dx + dy * dy).sqrt() / c_d / (0.0147 * (6.0 + 44.0 * p[P::CtH]) * 0.05);
    if p[P::Glow] > 0.0 && p[P::CtW] >= 0.001 {
        if let Some(r) = sc.band(9.0 / dome_k, -0.012 - px0) {
            sc.set_uniforms(cx, &mut sp.glow.draw_vars);
            sp.glow.draw_vars.set_uniform(cx, live_id!(dome_k), &[dome_k as f32]);
            sp.glow.draw_abs(cx, r);
        }
    }
    // the ranges, far to near, each over its height band, its props just behind it
    sc.set_uniforms(cx, &mut sp.range.draw_vars);
    for k in 0..5 {
        props.draw(cx, sc, Bucket::Range(k));
        if let Some(r) = sc.band(band_top(sc, k), -2.0 * px0) {
            sp.range.k = k as f32;
            sp.range.hk = (p[P::MtH] * mt_scale(k)) as f32;
            sp.range.draw_abs(cx, r);
        }
    }
    props.draw(cx, sc, Bucket::BehindCity);
    city.draw_blit(cx, sc);
    props.draw(cx, sc, Bucket::FrontOfCity);
    if sc.mirror {
        return;
    }
    if let Some(r) = sc.band(band_top(sc, 5), -2.0 * px0) {
        sp.range.k = 5.0;
        sp.range.draw_abs(cx, r);
    }
    // the ground band: from the top of the near rise (raw elevation) down; the sea is all below the horizon
    let rise = (0..v.w).map(|x| sc.col(x, 6)).map(|g| g[1] as f64 / (g[2] as f64).max(1e-3)).fold(0.0, f64::max);
    if let Some(r) = sc.band(rise + 2.0 * px0, -10.0) {
        sc.set_uniforms(cx, &mut sp.ground.draw_vars);
        sp.ground.draw_abs(cx, r);
    }
    props.draw(cx, sc, Bucket::OverSea);
}

/// The highest raw elevation range k (5: the front treeline) reaches in this view: its ridge, its
/// canopy, its tallest single tree, plus two pixels.
fn band_top(sc: &SceneCtx, k: usize) -> f64 {
    let hmax = if k >= 2 { terrain::tree_hmax(sc.p, k - 2) } else { 0.0 };
    let mut top: f64 = 0.0;
    for x in 0..sc.view.w {
        let r = sc.col(x, k);
        let t = sc.col(x, 6)[2] as f64;
        let mut h = r[0] as f64;
        if k >= 2 && r[3] > 0.0 {
            h = h.max(r[2] as f64).max(r[1] as f64 + hmax);
        }
        top = top.max(h / t.max(1e-3));
    }
    top + 2.0 * sc.view.px0()
}
