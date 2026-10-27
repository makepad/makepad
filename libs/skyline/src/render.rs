//! The skyline core: one view of the 360 degree night band, drawn as a chain of passes.
//!
//! ```text
//!   producers        trees (coverage, one channel per tree layer)    stage B
//!                    city  (march: colour+alpha, emission+depth)      stage A
//!   scene pass       two RGBA16F targets (a band's three: the distance), every instance premultiplied-over in painter's order:
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
use crate::light::SkyLight;
use crate::params::{Params, P, SIL};
use crate::post::Post;
use crate::props::{Bucket, Props};
use crate::rng::TAU;
use crate::shaders::*;
use crate::terrain::{self, Trees, ROWS, TEX_W};
use crate::zones::{mt_dist, mt_scale, Zones};
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
    /// The first azimuth (0..1 = 0..360 degrees), the share of the turn shown, the size in pixels and
    /// the horizon line (a fraction of the height from the top); the margin (px a side) the scene shaders leave out of
    /// their arithmetic, which stays the page's (the water's, `Post::scene_view`).
    pub u0: f64, pub span: f64, pub w: usize, pub h: usize, pub horizon: f64, pub pad: usize,
}

impl ViewSpec {
    /// The full turn, `w` pixels wide.
    pub fn strip(p: &Params, w: usize) -> Self { Self::new(p, 0.0, 1.0, w) }
    /// The detail view: `zoom` of the turn centred on `pan`.
    pub fn detail(p: &Params, w: usize) -> Self { Self::new(p, p[P::Pan] - p[P::Zoom] / 2.0, p[P::Zoom], w) }
    /// The rows: the field of view times Size (`crate::band_geom`).
    pub fn new(p: &Params, u0: f64, span: f64, w: usize) -> Self {
        let (w, g) = (w.max(8), crate::band_geom(p));
        let h = ((w as f64 * g.deg / (span * 360.0)).round() as usize).max(8);
        Self { u0, span, w, h, horizon: g.horizon, pad: 0 }
    }
    /// Radians per pixel of the raw view.
    pub fn px0(&self) -> f64 { self.span * TAU / self.w as f64 }
    /// Pass row of raw elevation `ang` (rad).
    pub fn row_of(&self, ang: f64) -> f64 { self.horizon * self.h as f64 - ang / self.px0() }
    /// The reflection run's view: the horizon moved by under a pixel, so the mirrored rows sample the elevations of the
    /// upright rows (the water reads the mirror on the upright grid, as the page reads its upright layers).
    pub fn mirrored(&self) -> Self { let y = self.horizon * self.h as f64; Self { horizon: (2.0 * y.round() - y) / self.h as f64, ..*self } }
}

/// A pass with RGBA16F targets cleared to transparent: two (colour + coverage, light + coverage), or one (`color` and
/// `light` are the same texture); for a band (`Skyline::draw_job`), one more after them, `dist`: the distance (m) of what
/// each pixel shows premultiplied by its coverage, and its light plane, (a D, L Dl / 1000, L, a) (`SkyScene::at_lit`),
/// the shaders' next output (the scene's fb2; the water's and the final's fb1).
pub struct SceneTarget { pub pass: DrawPass, pub list: DrawList2d, pub color: Texture, pub light: Texture, pub dist: Option<Texture> }

impl SceneTarget {
    pub fn new(cx: &mut Cx, name: &str, two: bool) -> Self { Self::with_dist(cx, name, two, false) }

    /// `new`, and with `dist` the distance target after the others.
    pub fn with_dist(cx: &mut Cx, name: &str, two: bool, dist: bool) -> Self {
        let pass = DrawPass::new_with_name(cx, name);
        let target = |cx: &mut Cx| {
            let t = Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
            pass.add_color_texture(cx, &t, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
            t
        };
        let color = target(cx);
        let light = if two { target(cx) } else { color.clone() };
        let dist = dist.then(|| target(cx));
        Self { list: DrawList2d::new(cx), pass, color, light, dist }
    }

    /// What a reader of this pass's distance binds: the distance target, or without one the light target in its place.
    /// Every pass of a view (or a file export) is without one, the readers' own passes too, so their distance output is
    /// dropped and what they read there goes nowhere.
    pub fn dist_in(&self) -> &Texture { self.dist.as_ref().unwrap_or(&self.light) }

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
    pub view: &'a ViewSpec, pub p: &'a Params,
    /// The sky's light, and its rings as a texture (`Skyline::ring_tex`); the sky the day's sea mirrors (`Skyline::sea_tex`).
    pub light: &'a SkyLight, pub ring: &'a Texture, pub sea: &'a Texture,
    /// The lakes (`Skyline::lake_tex`).
    pub lakes: &'a Texture,
    /// The host's aurora for the night water (`Skyline::aurora_tex`), when it hands one (plan hdri-7 D12).
    pub aurora: Option<&'a Texture>,
    /// The placement zones (the city and the props read them).
    pub z: &'a Zones,
    /// The column data texture, its geometry (width, height, margin, count) and the data on the CPU (see [`terrain::ROWS`]).
    pub cols: &'a Texture, pub col_geom: [f32; 4], pub col_data: &'a [[f32; 4]],
    /// The single trees' coverage (or a blank texture).
    pub tree_cov: Texture,
    /// The city march's targets once stage A provides them: colour + alpha, emission + depth (m).
    pub city: Option<(Texture, Texture)>,
    /// The producers' passes (trees, city): a pass that samples them puts them under itself, so they paint first.
    pub inputs: [Option<DrawPassId>; 2],
    /// The reflection run: everything seen mirrored about the horizon (or its own waterline); the upright run's horizon
    /// row less this run's (px; see [`ViewSpec::mirrored`]).
    pub mirror: bool, pub up_dy: f64,
    /// A band's picture: the passes keep the distance target (`SceneTarget::dist`).
    pub dist: bool,
}

impl SceneCtx<'_> {
    /// Every parameter (by its key), the view, the sky's light and the shared textures onto a scene shader.
    pub fn set_uniforms(&self, cx: &Cx, vars: &mut DrawVars) {
        self.p.set_uniforms(cx, vars);
        let (v, m) = (self.view, self.view.pad as f64);
        vars.set_uniform(cx, live_id!(view_size), &[v.w as f32, v.h as f32]);
        vars.set_uniform(cx, live_id!(view_span), &[(v.u0 + m * v.span / v.w as f64) as f32, (v.span - 2.0 * m * v.span / v.w as f64) as f32]);
        vars.set_uniform(cx, live_id!(view_horizon), &[(v.horizon + m * (2.0 * v.horizon - 1.0) / (v.h as f64 - 2.0 * m)) as f32]);
        vars.set_uniform(cx, live_id!(view_pad), &[m as f32]);
        vars.set_uniform(cx, live_id!(col_geom), &self.col_geom);
        vars.set_uniform(cx, live_id!(mirror), &[if self.mirror { 1.0 } else { 0.0 }]);
        vars.set_uniform(cx, live_id!(up_dy), &[self.up_dy as f32]);
        let l = self.light;
        // the night silhouettes' designed colour, in the host's units
        vars.set_uniform(cx, live_id!(sil), &SIL.map(|c| c * l.gpu_gain()));
        vars.set_uniform(cx, live_id!(sun_u), &[l.sun_u]);
        vars.set_uniform(cx, live_id!(sun_el), &[l.sun_el_deg.to_radians()]);
        vars.set_uniform(cx, live_id!(sun_rgb), &l.sun_rgb);
        vars.set_uniform(cx, live_id!(ambient_rgb), &l.ambient_rgb);
        vars.set_uniform(cx, live_id!(lit), &[l.lights_factor()]);
        vars.set_uniform(cx, live_id!(design_gain), &[l.gpu_gain()]);
        vars.set_uniform(cx, live_id!(host_ground), &[if l.host_ground { 1.0 } else { 0.0 }]);
        vars.set_uniform(cx, live_id!(sea_edge), &[crate::sea_edge_rad(self.p) as f32]);
        vars.set_uniform(cx, live_id!(lake_n), &[self.z.lakes.len() as f32]);
        vars.set_uniform(cx, live_id!(lake_eye), &[(crate::props::kit::EYE * self.p[P::Size]) as f32]);
        vars.set_uniform(cx, live_id!(near_rise), &[crate::NEAR_RISE_M as f32]);
        vars.set_uniform(cx, live_id!(wet), &[l.own_wet()]);
        vars.set_uniform(cx, live_id!(host_wet), &[l.floor_share()]);
        vars.set_uniform(cx, live_id!(point_core), &l.point_core());
        bind_texture(cx, vars, live_id!(lake_tex), self.lakes);
        bind_texture(cx, vars, live_id!(ring_tex), self.ring);
        bind_texture(cx, vars, live_id!(cols), self.cols);
        bind_texture(cx, vars, live_id!(tree_cov), &self.tree_cov);
    }

    /// The column data of output column x (0..w) and row r.
    pub fn col(&self, x: usize, r: usize) -> [f32; 4] { self.col_data[(x + self.col_geom[2] as usize) * ROWS + r] }
    /// The top of the near ground rise in this view (raw elevation, rad).
    pub fn rise(&self) -> f64 { (0..self.view.w).map(|x| self.col(x, 6)).map(|g| g[1] as f64 / (g[2] as f64).max(1e-3)).fold(0.0, f64::max) }

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
fn ms_since(t0: f64) -> f64 { (Cx::monotonic_now() - t0) * 1000.0 }

/// Binds `texture` to the shader's texture input `name`, if it has one.
pub fn bind_texture(cx: &Cx, vars: &mut DrawVars, name: LiveId, texture: &Texture) {
    if let Some(slot) = vars.draw_shader_id.and_then(|id| cx.draw_shaders[id.index].mapping.textures.iter().position(|t| t.id == name)) {
        vars.set_texture(slot, texture);
    }
}

/// What the last draws of a view cost.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Size of the last view drawn; CPU ms of the zones, the columns (when rebuilt) and recording the passes; how many recordings.
    pub w: usize, pub h: usize, pub zones_ms: f64, pub cols_ms: f64, pub record_ms: f64, pub renders: u64,
}

pub struct ColumnCache { key: u64, m: usize, data: Vec<[f32; 4]>, tex: Texture, geom: [f32; 4] }

/// The parameters the zones depend on (with the seed).
const ZONE_KEYS: &[P] = &[
    P::CtC, P::CtW, P::CtR, P::Land, P::LandC, P::PrInd, P::PrTown, P::PrBridge, P::PrFair, P::PrTurb, P::PrMast, P::MtH, P::MtW, P::MtC,
    P::MtRug, P::TrC, P::TrW, P::TrH, P::Size, P::PrLake,
];

#[derive(Script, ScriptHook)]
pub struct Skyline {
    #[live] draw_glow: DrawSkyGlow,
    #[live] draw_range: DrawSkyRange,
    #[live] draw_ground: DrawSkyGround,
    #[live] pub trees: Trees,
    #[live] pub city: City,
    #[live] pub props: Props,
    #[live] pub post: Post,
    #[rust] scene: Option<SceneTarget>,
    #[rust] mirror: Option<SceneTarget>,
    #[rust] pub zones: Option<(u64, Zones)>,
    #[rust] pub cols: Option<ColumnCache>,
    #[rust] blank: Option<Texture>,
    /// The sky's horizon rings and the host's ground (`SkyLight::ring_texels`) and, with `host_ground`, the coast
    /// (`coast_ring`), and the hash of the light and the zones they were written for.
    #[rust] ring_tex: Option<(u64, Texture)>,
    /// The sky the day's sea mirrors (`SkyLight::sea_texels`), written with `ring_tex`.
    #[rust] sea_tex: Option<Texture>,
    /// The lakes (`lakes::texels`), and the zones' key they were written for.
    #[rust] lake_tex: Option<(u64, Texture)>,
    /// The host's aurora (`SkyLight::aurora_texels`), written with `ring_tex`; None without one.
    #[rust] aurora_tex: Option<Texture>,
    /// The key of the picture the targets hold.
    #[rust] pub drawn: Option<u64>,
    #[rust] pub stats: Stats,
    /// A running export or band (`export.rs`), on a Skyline of its own.
    #[rust] pub job: Option<crate::export::Job>,
}

impl Skyline {
    /// Draws `view` into `rect` of the current pass, lit by `light`: records the passes when the picture changed,
    /// then the final composite (`tone`: the display tone shoulder; an export keeps the raw HDR).
    /// The scene has the blur's margin, so lights at the edges blur with their real neighbours; the final crops it.
    pub fn draw_view(&mut self, cx: &mut Cx2d, rect: Rect, shown: &ViewSpec, p: &Params, tone: bool, light: &SkyLight) {
        self.draw_view_with(cx, rect, shown, p, tone, light, false);
    }

    /// `draw_view`; with `dist` (a band's tile, `draw_job`) the passes keep the distance of what each pixel shows
    /// (`SceneTarget::dist`), and the final writes it as its second output. Targets made without it are made again.
    pub(crate) fn draw_view_with(&mut self, cx: &mut Cx2d, rect: Rect, shown: &ViewSpec, p: &Params, tone: bool, light: &SkyLight, dist: bool) {
        if self.scene.as_ref().is_some_and(|s| s.dist.is_some() != dist) { self.release(); }
        let view = &self.post.scene_view(p, shown, light);
        self.update_inputs(cx, view, p, light);
        // the picture: every parameter but the view's own (pan, zoom and the field of view only
        // place the view, which its first azimuth, span and size say), the view and the sky's light
        let view_only = [P::Pan as usize, P::Zoom as usize, P::Vfov as usize];
        let mut picture: Vec<f64> = p.v.iter().enumerate().filter(|(i, _)| !view_only.contains(i)).map(|(_, x)| *x).collect();
        picture.extend([view.u0, view.span, view.w as f64, view.h as f64, view.horizon, f64::from_bits(light.hash())]);
        let key = p.hash(&[], &picture);
        if self.scene.is_none() { self.scene = Some(SceneTarget::with_dist(cx, "skyline_scene", true, dist)); }
        if self.drawn != Some(key) {
            let t1 = Cx::monotonic_now();
            self.record(cx, view, p, light);
            (self.drawn, self.stats.renders, self.stats.record_ms) = (Some(key), self.stats.renders + 1, ms_since(t1));
        }
        let (scene, mirror) = (self.scene.as_ref().unwrap(), self.mirror.as_ref());
        self.post.draw_final(cx, rect, &crate::post::SceneOut { view, scene, mirror }, tone);
        (self.stats.w, self.stats.h) = (shown.w, shown.h);
    }

    /// Lets go of every target (an export core between jobs), and of the shaders' hold on the textures.
    pub fn release(&mut self) {
        (self.scene, self.mirror, self.drawn, self.props.pl) = (None, None, None, None);
        (self.post.water, self.post.blur, self.post.water_dist, self.post.dist_blur) = (None, None, None, None);
        self.city.release(); self.trees.release();
        let (pr, ci, po) = (&mut self.props, &mut self.city, &mut self.post);
        for v in [&mut self.draw_glow.draw_vars, &mut self.draw_range.draw_vars, &mut self.draw_ground.draw_vars, &mut self.trees.draw_trees.draw_vars,
            &mut ci.draw_city.draw_vars, &mut ci.draw_blit.draw_vars, &mut pr.draw_part.draw_vars, &mut pr.draw_blit.draw_vars,
            &mut po.draw_final.draw_vars, &mut po.draw_blur.draw_vars, &mut po.draw_water.draw_vars, &mut po.draw_water_day.draw_vars,
            &mut po.draw_water_dist.draw_vars, &mut po.draw_dist_across.draw_vars, &mut po.draw_dist_down.draw_vars, &mut po.draw_aurora_water.draw_vars] {
            v.texture_slots = Default::default();
        }
    }

    /// The zones, the columns for this view and the sky's rings, rebuilt when what they depend on changed.
    fn update_inputs(&mut self, cx: &mut Cx, view: &ViewSpec, p: &Params, light: &SkyLight) {
        let zkey = p.hash(ZONE_KEYS, &[]);
        if self.zones.as_ref().map(|z| z.0) != Some(zkey) {
            let t = Cx::monotonic_now();
            (self.zones, self.stats.zones_ms) = (Some((zkey, Zones::new(p))), ms_since(t));
        }
        let z = &self.zones.as_ref().unwrap().1;
        if self.lake_tex.as_ref().map(|t| t.0) != Some(zkey) {
            let format = TextureFormat::VecRGBAf32 { width: crate::lakes::LAKES_MAX, height: 1, data: Some(crate::lakes::texels(&z.lakes)), updated: TextureUpdated::Full };
            let tex = match self.lake_tex.take() { Some((_, t)) => { *t.get_format(cx) = format; t } None => Texture::new_with_format(cx, format) };
            self.lake_tex = Some((zkey, tex));
        }
        let m = terrain::margin(p, view.span, view.w);
        let ckey = p.hash(terrain::COLUMN_KEYS, &[view.u0, view.span, view.w as f64, m as f64]);
        if self.cols.as_ref().map(|c| c.key) != Some(ckey) {
            let t = Cx::monotonic_now();
            let data = terrain::columns(z, p, view.u0, view.span, view.w, m);
            let n = view.w + 2 * m;
            let (tw, bands) = (n.min(TEX_W), n.div_ceil(n.min(TEX_W)));
            let mut texels = vec![0f32; tw * ROWS * bands * 4];
            for (x, col) in data.chunks(ROWS).enumerate() {
                let (b, xx) = (x / tw, x % tw);
                for (r, v) in col.iter().enumerate() {
                    let o = ((r + ROWS * b) * tw + xx) * 4;
                    texels[o..o + 4].copy_from_slice(v);
                }
            }
            let format = TextureFormat::VecRGBAf32 { width: tw, height: ROWS * bands, data: Some(texels), updated: TextureUpdated::Full };
            let tex = match self.cols.take() { Some(c) => { *c.tex.get_format(cx) = format; c.tex } None => Texture::new_with_format(cx, format) };
            let geom = [tw as f32, (ROWS * bands) as f32, m as f32, n as f32];
            self.cols = Some(ColumnCache { key: ckey, m, data, tex, geom });
            self.stats.cols_ms = ms_since(t);
        }
        if self.blank.is_none() {
            self.blank = Some(Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: 1, height: 1, data: Some(vec![0.0; 4]), updated: TextureUpdated::Full }));
        }
        let lkey = light.hash() ^ zkey.rotate_left(17);
        if self.ring_tex.as_ref().map(|r| r.0) != Some(lkey) {
            let mut texels = light.ring_texels();
            // row 3: the coast, which only a host's ground reads
            let coast = if light.host_ground { crate::light::coast_ring(|u| z.land(u)) } else { [0.0; crate::light::RING] };
            texels.extend(coast.iter().flat_map(|c| [*c, 0.0, 0.0, 1.0]));
            let format = TextureFormat::VecRGBAf32 { width: crate::light::RING, height: 4, data: Some(texels), updated: TextureUpdated::Full };
            let tex = match self.ring_tex.take() { Some((_, t)) => { *t.get_format(cx) = format; t } None => Texture::new_with_format(cx, format) };
            self.ring_tex = Some((lkey, tex));
            let format = TextureFormat::VecRGBAf32 { width: crate::light::RING, height: 2, data: Some(light.sea_texels()), updated: TextureUpdated::Full };
            let sea = match self.sea_tex.take() { Some(t) => { *t.get_format(cx) = format; t } None => Texture::new_with_format(cx, format) };
            self.sea_tex = Some(sea);
            // the host's aurora for the night water (plan hdri-7 D12): two rows like the sea's; none without one
            let old = self.aurora_tex.take();
            self.aurora_tex = light.aurora_texels().map(|texels| {
                let format = TextureFormat::VecRGBAf32 { width: crate::light::RING, height: 2, data: Some(texels), updated: TextureUpdated::Full };
                match old {
                    Some(t) => {
                        *t.get_format(cx) = format;
                        t
                    }
                    None => Texture::new_with_format(cx, format),
                }
            });
        }
    }

    /// Records every pass of the view: the producers, the scene, the mirror, the post passes.
    fn record(&mut self, cx: &mut Cx2d, view: &ViewSpec, p: &Params, light: &SkyLight) {
        let z = &self.zones.as_ref().unwrap().1;
        let cols = self.cols.as_ref().unwrap();
        debug_assert_eq!(cols.m, cols.geom[2] as usize);
        let blank = self.blank.as_ref().unwrap();
        let ring = &self.ring_tex.as_ref().unwrap().1;
        let sea = self.sea_tex.as_ref().unwrap();
        let lakes = &self.lake_tex.as_ref().unwrap().1;
        let mut scene = self.scene.take().unwrap();
        let (scene_id, dist) = (scene.pass.draw_pass_id(), scene.dist.is_some());
        let mv = view.mirrored();
        let mut sc = SceneCtx { view, p, light, ring, sea, lakes, aurora: self.aurora_tex.as_ref(), z, cols: &cols.tex, col_geom: cols.geom, col_data: &cols.data, tree_cov: blank.clone(), city: None, inputs: [None; 2], mirror: false, up_dy: 0.0, dist };
        // producers: children of the scene pass, which samples them
        self.trees.draw_pass(cx, &sc, scene_id);
        self.city.draw_pass(cx, &sc, scene_id);
        sc.tree_cov = self.trees.coverage().unwrap_or(blank).clone();
        (sc.city, sc.inputs) = (self.city.textures(), [self.trees.pass_id(), self.city.pass_id()]);
        self.props.update(z, p);
        // the scene, and its mirror image when the water reflects it and the view shows sea (or there are lakes, or the
        // ground is wet)
        let parts = ScenePainter { glow: &mut self.draw_glow, range: &mut self.draw_range, ground: &mut self.draw_ground };
        let mut parts = (parts, &mut self.props, &mut self.city);
        scene.record(cx, view.w, view.h, |cx| draw_scene(cx, &sc, &mut parts));
        let sea = (0..view.w).any(|x| sc.col(x, 6)[0] < 0.35) || !z.lakes.is_empty() || light.own_wet() > 0.0;
        self.mirror = (self.post.wants_mirror(p, light) && sea).then(|| {
            let mut mirror = self.mirror.take().unwrap_or_else(|| SceneTarget::with_dist(cx, "skyline_mirror", true, dist));
            (sc.mirror, sc.view, sc.up_dy) = (true, &mv, (view.horizon - mv.horizon) * view.h as f64);
            mirror.record(cx, view.w, view.h, |cx| draw_scene(cx, &sc, &mut parts));
            (sc.mirror, sc.view, sc.up_dy) = (false, view, 0.0);
            mirror
        });
        self.post.draw_passes(cx, &sc, &crate::post::SceneOut { view, scene: &scene, mirror: self.mirror.as_ref() });
        self.scene = Some(scene);
    }
}

/// The foundation's own scene shaders.
struct ScenePainter<'a> { glow: &'a mut DrawSkyGlow, range: &'a mut DrawSkyRange, ground: &'a mut DrawSkyGround }

/// The scene list in painter's order (far to near). The mirror run leaves out what stands below
/// the eye's own horizon: the front treeline and the ground band; under a wet light it keeps them above the horizon.
fn draw_scene(cx: &mut Cx2d, sc: &SceneCtx, (sp, props, city): &mut (ScenePainter, &mut Props, &mut City)) {
    let (v, p) = (sc.view, sc.p);
    let px0 = v.px0();
    // the glow dome over downtown, behind everything
    let dome_k = dome_k(p);
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
            // the range's own height (Size times the base's): its valley haze keeps to the same share of it; its distance
            (sp.range.k, sp.range.hk, sp.range.dk) = (k as f32, (p[P::MtH] * mt_scale(k) * p[P::Size]) as f32, mt_dist(k as f64) as f32);
            sp.range.draw_abs(cx, r);
        }
    }
    props.draw(cx, sc, Bucket::BehindCity);
    city.draw_blit(cx, sc);
    props.draw(cx, sc, Bucket::FrontOfCity);
    // the reflection run ends here for the sea and the lakes. A wet ground (`SkyLight::wet`) lies in front of the near rise,
    // so under a wet light it goes on with the front treeline and the rise as they stand above it (to the ranges' two
    // pixels under the horizon), which the sea and the lakes then mirror too (not over a host's ground, whose own floor
    // holds its water: `SkyLight::own_wet`)
    let wet = sc.mirror && sc.light.own_wet() > 0.0;
    if sc.mirror && !wet { return; }
    if let Some(r) = sc.band(band_top(sc, 5), -2.0 * px0) { (sp.range.k, sp.range.dk) = (5.0, crate::NEAR_RISE_M as f32); sp.range.draw_abs(cx, r); }
    // the ground band: from the top of the near rise (raw elevation) down; the sea is all below the horizon
    if let Some(r) = sc.band(sc.rise() + 2.0 * px0, if wet { -2.0 * px0 } else { -10.0 }) {
        sc.set_uniforms(cx, &mut sp.ground.draw_vars);
        sp.ground.draw_abs(cx, r);
    }
    if wet { return; }
    props.draw(cx, sc, Bucket::OverSea);
}

/// The highest raw elevation (rad) range k (5: the front treeline) reaches over `w` columns (`col(x, r)`: column x's row
/// r, `terrain::ROWS`): its ridge, its canopy, its tallest single tree, over the column's off-centre factor.
pub(crate) fn range_top(p: &Params, w: usize, col: &dyn Fn(usize, usize) -> [f32; 4], k: usize) -> f64 {
    let hmax = if k >= 2 { terrain::tree_hmax(p, k - 2) } else { 0.0 };
    (0..w).fold(0.0f64, |top, x| {
        let r = col(x, k);
        let h = if k >= 2 && r[3] > 0.0 { (r[0] as f64).max(r[2] as f64).max(r[1] as f64 + hmax) } else { r[0] as f64 };
        top.max(h / (col(x, 6)[2] as f64).max(1e-3))
    })
}

/// The highest raw elevation range k reaches in this view (`range_top`), plus two pixels.
fn band_top(sc: &SceneCtx, k: usize) -> f64 { range_top(sc.p, sc.view.w, &|x, r| sc.col(x, r), k) + 2.0 * sc.view.px0() }

/// The glow dome's e-folding rate in elevation: the camera's distance to downtown over the tower height (the towers Size
/// times as tall: a dome Size times as high).
pub(crate) fn dome_k(p: &Params) -> f64 {
    let c_d = 28.0 * p[P::CtR];
    let dx = (p[P::CtC] * TAU).cos() * c_d - 0.8 * p[P::Off] * c_d * (p[P::OffDir] * TAU).cos();
    let dy = (p[P::CtC] * TAU).sin() * c_d - 0.8 * p[P::Off] * c_d * (p[P::OffDir] * TAU).sin();
    (dx * dx + dy * dy).sqrt() / c_d / (0.0147 * (6.0 + 44.0 * p[P::CtH]) * 0.05) / p[P::Size]
}
