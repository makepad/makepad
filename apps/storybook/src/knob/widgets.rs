//! The knob engine's widgets: `TurnedKnob`, one knob of a style in a
//! material in its own quad, turned by a drag; `KnobView3d`, the same solid
//! ray marched under an orbiting camera; and the bake cache they share.
use super::bake::{self, GeomBake, GeomConsts, GeomKey, ShadeConsts, ShadeKey, DATA_ROWS, KNOT_FLOATS, SHADE_N, TAPS};
use super::presets::{style_shapes, KnobMaterial, KnobShape, MATERIALS, STYLES};
use super::shader::{DrawKnobView3d, DrawTurnedKnob};
use crate::makepad_widgets::*;
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.storybook.TurnedKnobBase = #(TurnedKnob::register_widget(vm))
    /** One knob of the bench's knob engine: `style` (0..17) in `material`
     * (0..12), turned to `value`. A drag turns it; a tap raises `Tapped`. */
    mod.storybook.TurnedKnob = set_type_default() do mod.storybook.TurnedKnobBase{
        width: 160.
        height: 160.
    }

    mod.storybook.KnobView3dBase = #(KnobView3d::register_widget(vm))
    /** The knob's solid ray marched in 3D. A drag orbits the camera,
     * ctrl and the wheel zoom, a double tap puts the camera back. */
    mod.storybook.KnobView3d = set_type_default() do mod.storybook.KnobView3dBase{
        width: 320.
        height: 230.
    }
}

/// A shape's geometry bake as the GPU holds it.
pub struct GeomAssets {
    pub key: GeomKey,
    pub data: Texture,
    pub consts: GeomConsts,
    bake: GeomBake,
}

/// A shape's shadow bake as the GPU holds it.
pub struct ShadeAssets {
    pub key: ShadeKey,
    pub knots: Texture,
    pub shade: Texture,
    pub consts: ShadeConsts,
}

/// Both bakes a knob draws with.
#[derive(Clone)]
pub struct KnobAssets {
    pub geom: Rc<GeomAssets>,
    pub shade: Rc<ShadeAssets>,
}

impl KnobAssets {
    /// Whether these are the bakes for the shape with this fingerprint in
    /// this material.
    pub fn fits(&self, shape: u64, m: &KnobMaterial) -> bool {
        self.shade.key == ShadeKey::new(shape, m)
    }
}

/// The bakes in use, newest last. A shape's curves are baked once per crease
/// blur and its shadows once per light, and every knob and view that draws
/// it shares them: moving the light re-bakes the shadows only.
#[derive(Default)]
struct BakeCache {
    geoms: Vec<Rc<GeomAssets>>,
    shades: Vec<Rc<ShadeAssets>>,
}

/// How many of each the cache keeps: every style under two lights, so a
/// gallery survives a light being moved back and forth.
///
/// A shape being edited makes a new key at every step of a slider's drag,
/// and a long drag pushes older bakes off this list. That does not thrash:
/// a knob holds the bakes it draws with and only looks here when its key
/// changes, so the knobs the drag does not reshape keep theirs and bake
/// nothing. What a drag can cost is a later light moved back finding its
/// old bakes gone, one bake per knob, once.
const CACHE_SIZE: usize = 48;

fn cached<T, K: PartialEq>(list: &mut Vec<Rc<T>>, key: &K, key_of: impl Fn(&T) -> &K) -> Option<Rc<T>> {
    let pos = list.iter().position(|a| key_of(a) == key)?;
    let hit = list.remove(pos);
    list.push(hit.clone());
    Some(hit)
}

fn keep<T>(list: &mut Vec<Rc<T>>, item: Rc<T>) {
    list.push(item);
    if list.len() > CACHE_SIZE {
        list.remove(0);
    }
}

/// The bakes for this shape in this material, from the cache or made now.
pub fn knob_assets(cx: &mut Cx, shape: &KnobShape, m: &KnobMaterial) -> KnobAssets {
    let print = shape.fingerprint();
    let gkey = GeomKey::new(print, m);
    let skey = ShadeKey::new(print, m);
    let geom = match cached(&mut cx.global::<BakeCache>().geoms, &gkey, |a| &a.key) {
        Some(g) => g,
        None => {
            let bake = bake::bake_geometry(shape, &gkey);
            let data = Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: TAPS,
                    height: DATA_ROWS,
                    data: Some(bake.data.clone()),
                    updated: TextureUpdated::Full,
                },
            );
            let g = Rc::new(GeomAssets { key: gkey, data, consts: bake.consts.clone(), bake });
            keep(&mut cx.global::<BakeCache>().geoms, g.clone());
            g
        }
    };
    let shade = match cached(&mut cx.global::<BakeCache>().shades, &skey, |a| &a.key) {
        Some(s) => s,
        None => {
            let bake = bake::bake_shade(shape, &geom.bake, &skey);
            let knots = Texture::new_with_format(
                cx,
                TextureFormat::VecRf32 {
                    width: KNOT_FLOATS,
                    height: 1,
                    data: Some(bake.knots),
                    updated: TextureUpdated::Full,
                },
            );
            let table = Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: SHADE_N,
                    height: SHADE_N,
                    data: Some(bake.shade),
                    updated: TextureUpdated::Full,
                },
            );
            let s = Rc::new(ShadeAssets { key: skey, knots, shade: table, consts: bake.consts });
            keep(&mut cx.global::<BakeCache>().shades, s.clone());
            s
        }
    };
    KnobAssets { geom, shade }
}

fn texture_slot(cx: &Cx, vars: &DrawVars, id: LiveId) -> Option<usize> {
    let shader = vars.draw_shader_id?;
    cx.draw_shaders[shader.index].mapping.textures.iter().position(|t| t.id == id)
}

fn u4(cx: &Cx, vars: &mut DrawVars, id: LiveId, v: [f64; 4]) {
    vars.set_uniform(cx, id, &[v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
}

fn ink4(c: u32) -> [f64; 4] {
    let v = bake::ink(c);
    [v[0], v[1], v[2], 1.0]
}

/// The material's uniforms, as the bench's `upload_uniforms` sets them.
pub fn set_material_uniforms(cx: &Cx, vars: &mut DrawVars, m: &KnobMaterial) {
    u4(cx, vars, live_id!(m_light), [m.lx, m.ly, m.lz.max(0.02), m.li]);
    u4(cx, vars, live_id!(m_relief), [m.bw, m.bc, m.raise, m.spec]);
    u4(cx, vars, live_id!(m_finish), [m.ao, m.rim, m.gloss, m.rough]);
    u4(cx, vars, live_id!(m_env), [m.env, m.persp, 2f64.powf(m.ev), m.roll]);
    u4(cx, vars, live_id!(m_surf), [m.metal, m.coat, m.coatr, m.envk]);
    u4(cx, vars, live_id!(m_studio), [bake::studio_lights(m), bake::studio_panes(m), 0.0, 0.0]);
    u4(cx, vars, live_id!(m_shadow), [m.shadow, m.sblur, m.fall, m.oao]);
    u4(cx, vars, live_id!(m_inner), [m.inner, m.inner_r, m.lip, m.glow]);
    u4(cx, vars, live_id!(m_tune), [m.level, m.sink, m.hair, m.aoreach]);
    u4(cx, vars, live_id!(m_knob), [m.pdepth, m.pfin, m.mbev, m.facegrad]);
    u4(cx, vars, live_id!(m_env_ref), bake::env_ref(m));
    u4(cx, vars, live_id!(m_ground), ink4(m.ground));
    u4(cx, vars, live_id!(m_body), ink4(m.body_ink));
    u4(cx, vars, live_id!(m_light_ink), ink4(m.light_ink));
    u4(cx, vars, live_id!(m_shadow_ink), ink4(m.shadow_ink));
    u4(cx, vars, live_id!(m_glow_ink), ink4(m.glow_ink));
    u4(cx, vars, live_id!(m_ptr_ink), ink4(m.ptr_ink));
}

/// The shape's uniforms and its bake's textures.
pub fn set_style_uniforms(cx: &Cx, vars: &mut DrawVars, s: &KnobShape, a: &KnobAssets) {
    let c = &a.geom.consts;
    let d = &a.shade.consts;
    u4(cx, vars, live_id!(knob_zero), [0.0; 4]);
    u4(cx, vars, live_id!(s_flute), [s.flutes, s.fd, s.fs, s.gtaper]);
    u4(cx, vars, live_id!(s_cap), [c.flute_r[0], c.flute_r[1], s.capr, s.spun]);
    u4(cx, vars, live_id!(s_cap_ink), ink4(s.cap_ink));
    u4(cx, vars, live_id!(s_ptr), [s.ptype, s.pr0, s.pr1, s.pw]);
    u4(cx, vars, live_id!(s_ticks), [s.ticks, s.tickr, s.tickl, s.tickw]);
    u4(cx, vars, live_id!(s_arc), [s.arcr, s.arcw, s.awell, s.well]);
    u4(cx, vars, live_id!(s_wing), [s.wr1 - s.wr0, s.wr0, s.wr1, s.wwmax]);
    u4(cx, vars, live_id!(s_wing2), [c.wing_top, c.wing_hc, s.barfil, s.flat]);
    u4(cx, vars, live_id!(s_wing_wc), [c.wing_wc[0], c.wing_wc[1], c.wing_ends[0], c.wing_ends[1]]);
    u4(cx, vars, live_id!(s_wing_geo), c.wing_geo);
    u4(cx, vars, live_id!(s_wing3), [c.wing_rc[0], c.wing_rc[1], s.wmode, s.wbase]);
    u4(cx, vars, live_id!(s_cut), [s.cut, s.cn, s.cr, s.cs]);
    u4(cx, vars, live_id!(s_cut2), [s.cl, s.cf, s.cw, s.cfil]);
    u4(cx, vars, live_id!(s_cut3), [s.csph, s.cz, c.wing_thru, if c.foot_notched { 1.0 } else { 0.0 }]);
    u4(cx, vars, live_id!(s_sil), [d.sil_n, d.sil_s, d.wk_n, d.wk_b]);
    u4(cx, vars, live_id!(s_wt), d.wt);
    u4(cx, vars, live_id!(s_pre), [c.prof09, c.prof_cut, 0.0, 0.0]);
    if let Some(slot) = texture_slot(cx, vars, live_id!(knob_data)) {
        vars.set_texture(slot, &a.geom.data);
    }
    if let Some(slot) = texture_slot(cx, vars, live_id!(knob_knots)) {
        vars.set_texture(slot, &a.shade.knots);
    }
    if let Some(slot) = texture_slot(cx, vars, live_id!(knob_self)) {
        vars.set_texture(slot, &a.shade.shade);
    }
}

/// How far from its centre a knob paints anything, in the bench's units (the
/// knob's radius is 56): its solid, the cast shadow and the ground lip with
/// their blur, the contact ring, a well's, the latched glow and the marks.
/// The same bounds the shader stops each of them at, so the quad the knob is
/// drawn in never cuts one short.
pub fn knob_reach(m: &KnobMaterial, s: &KnobShape, a: &KnobAssets, lit: bool) -> f64 {
    const R: f64 = 56.0;
    let wr = s.wr0.abs().max(s.wr1.abs());
    let mut solid_r = R * wr.max(1.0);
    if s.wr1 - s.wr0 > 0.01 {
        solid_r += s.wwmax * R * 0.5;
    }
    if s.flutes >= 1.0 {
        solid_r += s.fd.abs();
    }
    let mut reach = solid_r;
    let blur = m.sblur.max(0.001);
    if m.level > 0.5 {
        // The shader's own bound for the knob's ground: the outline, the
        // cast shadow's length, four blurs and the lip's offset.
        let tanel = m.lz.max(0.05) / m.lx.hypot(m.ly).max(0.05);
        let hk = R * m.pdepth.max(0.001);
        let off = (m.raise * 1.2).max(0.0) / tanel;
        let cast_l = a.geom.consts.wing_top.max(1.0) * hk / tanel;
        reach = reach.max(solid_r + cast_l + 4.0 * blur + off);
        // The wells' contact rings.
        if s.well > 1.01 {
            reach = reach.max(s.well * R + 4.0 * blur);
        }
        if s.awell > 0.01 && s.arcw >= 0.01 {
            reach = reach.max(s.arcr * R + s.arcw * s.awell * 0.5 + 4.0 * blur);
        }
    }
    if lit && m.glow > 0.001 {
        reach = reach.max(solid_r + 4.0 * m.glow * 14.0);
    }
    // The marks, with the room the shader gives an LED's halo.
    let mut mr: f64 = 0.0;
    if s.ticks >= 1.0 {
        mr = s.tickr + s.tickl;
    }
    if s.ptype > 0.5 {
        mr = mr.max(s.pr1);
    }
    if s.arcw >= 0.01 {
        mr = mr.max(s.arcr);
    }
    reach = reach.max(R * mr + s.arcw + (s.pw.max(s.tickw) * R / 56.0).max(1.2) + 42.0);
    // And a few units for the antialiasing.
    reach + 4.0
}

/// What a knob raised.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum TurnedKnobAction {
    /// Turned to this value by a drag.
    Changed(f64),
    /// Pressed and let go without turning.
    Tapped,
    #[default]
    None,
}

/// A drag in progress: the value and the point it started from.
#[derive(Clone, Copy)]
struct Drag {
    value: f64,
    last: Vec2d,
    travelled: f64,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TurnedKnob {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_knob: DrawTurnedKnob,
    /// The style, 0..17, in the order of `STYLES`, unless the host has
    /// handed a shape over with `set_shape`.
    #[live]
    pub style: f64,
    /// The material, 0..12, in the order of `MATERIALS`, unless the host
    /// has handed one over with `set_material`.
    #[live]
    pub material: f64,
    /// The value, 0..1 over the 270 degree sweep.
    #[live(0.34)]
    pub value: f64,
    /// Latched: the LED marks and the glow lit.
    #[live]
    pub lit: bool,
    /// The knob's radius as a fraction of half the quad's shorter side.
    #[live(0.5)]
    pub fill: f64,
    /// The knob's radius in the bench's units. Every other length (blur,
    /// bevels, strokes) is in those units, so a knob keeps its proportions
    /// at any size; the bench's own knob is 56.
    #[live(56.0)]
    pub unit_radius: f64,
    /// The last points of the quad fade the ground's shading out, so a
    /// shadow the quad cuts short ends softly.
    #[live(8.0)]
    pub edge_fade: f64,
    /// Whether a drag turns it.
    #[live(true)]
    pub turnable: bool,
    #[rust]
    custom: Option<KnobMaterial>,
    #[rust]
    custom_shape: Option<KnobShape>,
    #[rust]
    assets: Option<KnobAssets>,
    #[rust]
    drag: Option<Drag>,
    /// The layout rect: what a hit is tested against and what the widget
    /// reports as its area. The quad it draws reaches further (see
    /// [`knob_reach`]).
    #[area]
    #[rust]
    hit: Area,
}

impl TurnedKnob {
    pub fn material(&self) -> KnobMaterial {
        self.custom.unwrap_or(MATERIALS[(self.material.round().max(0.0) as usize).min(MATERIALS.len() - 1)])
    }

    pub fn style_index(&self) -> usize {
        (self.style.round().max(0.0) as usize).min(STYLES.len() - 1)
    }

    /// The shape it draws: the one handed over, or its style's.
    pub fn shape(&self) -> KnobShape {
        self.custom_shape.clone().unwrap_or_else(|| style_shapes()[self.style_index()].clone())
    }

    pub fn set_material(&mut self, cx: &mut Cx, m: &KnobMaterial) {
        if self.custom.as_ref() != Some(m) {
            self.custom = Some(*m);
            self.draw_knob.redraw(cx);
        }
    }

    /// Draw this shape rather than the style's.
    pub fn set_shape(&mut self, cx: &mut Cx, shape: &KnobShape) {
        if self.custom_shape.as_ref() != Some(shape) {
            self.custom_shape = Some(shape.clone());
            self.draw_knob.redraw(cx);
        }
    }

    pub fn set_style(&mut self, cx: &mut Cx, style: usize) {
        if self.style_index() != style {
            self.style = style as f64;
            self.draw_knob.redraw(cx);
        }
    }

    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        let value = value.clamp(0.0, 1.0);
        if self.value != value {
            self.value = value;
            self.draw_knob.redraw(cx);
        }
    }

    pub fn set_lit(&mut self, cx: &mut Cx, lit: bool) {
        if self.lit != lit {
            self.lit = lit;
            self.draw_knob.redraw(cx);
        }
    }

    /// The draw shader this knob was built with: whether it is ready to
    /// draw is the backend's to say (`Cx::is_draw_shader_window_ready`).
    pub fn draw_shader_id(&self) -> Option<DrawShaderId> {
        self.draw_knob.draw_vars.draw_shader_id
    }
}

impl Widget for TurnedKnob {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let m = self.material();
        let shape = self.custom_shape.as_ref().unwrap_or(&style_shapes()[self.style_index()]);
        let print = shape.fingerprint();
        let assets = match &self.assets {
            Some(a) if a.fits(print, &m) => a.clone(),
            _ => {
                let a = knob_assets(cx, shape, &m);
                self.assets = Some(a.clone());
                a
            }
        };
        cx.add_rect_area(&mut self.hit, rect);
        let radius = self.fill * rect.size.x.min(rect.size.y) * 0.5;
        // The quad reaches as far as anything the knob paints on the ground,
        // past the layout rect where that is further: it composes over what
        // is under it (`knob_out` in the shader), so it needs no edge.
        let reach = knob_reach(&m, shape, &assets, self.lit) * radius.max(1.0) / self.unit_radius.max(1.0);
        let half = dvec2((rect.size.x * 0.5).max(reach), (rect.size.y * 0.5).max(reach));
        let quad = Rect { pos: rect.pos + rect.size * 0.5 - half, size: half * 2.0 };
        let vars = &mut self.draw_knob.draw_vars;
        set_material_uniforms(cx, vars, &m);
        set_style_uniforms(cx, vars, shape, &assets);
        u4(cx, vars, live_id!(k_state), [self.value, if self.lit { 1.0 } else { 0.0 }, self.edge_fade, 0.0]);
        u4(cx, vars, live_id!(k_geom), [half.x, half.y, radius.max(1.0), self.unit_radius]);
        self.draw_knob.draw_abs(cx, quad);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        match event.hits(cx, self.hit) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if fe.tap_count == 2 && self.turnable {
                    self.set_value(cx, 0.34);
                    cx.widget_action(uid, TurnedKnobAction::Changed(self.value));
                }
                self.drag = Some(Drag { value: self.value, last: fe.abs, travelled: 0.0 });
                if self.turnable {
                    cx.set_cursor(MouseCursor::Grabbing);
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(mut drag) = self.drag {
                    let delta = fe.abs - drag.last;
                    drag.last = fe.abs;
                    drag.travelled += delta.x.abs() + delta.y.abs();
                    if self.turnable {
                        // Up and right turn it up; a full sweep is a drag
                        // of four radii, whatever the knob's size.
                        let span = (fe.rect.size.x.min(fe.rect.size.y) * self.fill * 2.0).max(60.0);
                        drag.value = (drag.value + (delta.x - delta.y) / span).clamp(0.0, 1.0);
                        if drag.value != self.value {
                            self.set_value(cx, drag.value);
                            cx.widget_action(uid, TurnedKnobAction::Changed(self.value));
                        }
                    }
                    self.drag = Some(drag);
                }
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                let still = self.drag.map(|d| d.travelled < 3.0).unwrap_or(true);
                self.drag = None;
                if fe.is_over && still {
                    cx.widget_action(uid, TurnedKnobAction::Tapped);
                }
                cx.set_cursor(MouseCursor::Hand);
            }
            _ => {}
        }
    }
}

impl TurnedKnobRef {
    pub fn set_material(&self, cx: &mut Cx, m: &KnobMaterial) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_material(cx, m);
        }
    }

    pub fn set_shape(&self, cx: &mut Cx, shape: &KnobShape) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_shape(cx, shape);
        }
    }

    pub fn set_style(&self, cx: &mut Cx, style: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_style(cx, style);
        }
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }

    pub fn set_lit(&self, cx: &mut Cx, lit: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_lit(cx, lit);
        }
    }

    pub fn draw_shader_id(&self) -> Option<DrawShaderId> {
        self.borrow().and_then(|inner| inner.draw_shader_id())
    }

    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        if let TurnedKnobAction::Changed(v) = actions.find_widget_action(self.widget_uid()).cast() {
            return Some(v);
        }
        None
    }

    pub fn tapped(&self, actions: &Actions) -> bool {
        matches!(actions.find_widget_action(self.widget_uid()).cast(), TurnedKnobAction::Tapped)
    }
}

/// The bench's camera at rest: yaw, elevation, zoom.
pub const CAMERA_HOME: [f64; 3] = [0.55, 0.62, 1.0];

#[derive(Script, ScriptHook, Widget)]
pub struct KnobView3d {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_view: DrawKnobView3d,
    #[live]
    pub style: f64,
    #[live]
    pub material: f64,
    #[live(0.34)]
    pub value: f64,
    /// Yaw, elevation (0.12..1.55) and zoom.
    #[live(0.55)]
    pub yaw: f64,
    #[live(0.62)]
    pub elevation: f64,
    #[live(1.0)]
    pub zoom: f64,
    /// Rays per pixel along each axis: 1 is the bench's single ray, 2 four
    /// rays on a rotated grid (anti-aliased silhouettes and rims, at four
    /// times the march). While the camera is being dragged it takes one.
    #[live(2.0)]
    pub samples: f64,
    #[rust]
    custom: Option<KnobMaterial>,
    #[rust]
    custom_shape: Option<KnobShape>,
    #[rust]
    assets: Option<KnobAssets>,
    #[rust]
    orbit: Option<(Vec2d, [f64; 2])>,
}

impl KnobView3d {
    pub fn material(&self) -> KnobMaterial {
        self.custom.unwrap_or(MATERIALS[(self.material.round().max(0.0) as usize).min(MATERIALS.len() - 1)])
    }

    pub fn style_index(&self) -> usize {
        (self.style.round().max(0.0) as usize).min(STYLES.len() - 1)
    }

    /// The shape it draws: the one handed over, or its style's.
    pub fn shape(&self) -> KnobShape {
        self.custom_shape.clone().unwrap_or_else(|| style_shapes()[self.style_index()].clone())
    }

    pub fn set_material(&mut self, cx: &mut Cx, m: &KnobMaterial) {
        if self.custom.as_ref() != Some(m) {
            self.custom = Some(*m);
            self.draw_view.redraw(cx);
        }
    }

    /// Draw this shape rather than the style's.
    pub fn set_shape(&mut self, cx: &mut Cx, shape: &KnobShape) {
        if self.custom_shape.as_ref() != Some(shape) {
            self.custom_shape = Some(shape.clone());
            self.draw_view.redraw(cx);
        }
    }

    pub fn set_style(&mut self, cx: &mut Cx, style: usize) {
        if self.style_index() != style {
            self.style = style as f64;
            self.draw_view.redraw(cx);
        }
    }

    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        let value = value.clamp(0.0, 1.0);
        if self.value != value {
            self.value = value;
            self.draw_view.redraw(cx);
        }
    }

    pub fn set_camera(&mut self, cx: &mut Cx, yaw: f64, elevation: f64, zoom: f64) {
        self.yaw = yaw;
        self.elevation = elevation.clamp(0.12, 1.55);
        self.zoom = zoom.clamp(0.4, 4.0);
        self.draw_view.redraw(cx);
    }
}

impl Widget for KnobView3d {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let m = self.material();
        let shape = self.custom_shape.as_ref().unwrap_or(&style_shapes()[self.style_index()]);
        let print = shape.fingerprint();
        let assets = match &self.assets {
            Some(a) if a.fits(print, &m) => a.clone(),
            _ => {
                let a = knob_assets(cx, shape, &m);
                self.assets = Some(a.clone());
                a
            }
        };
        let vars = &mut self.draw_view.draw_vars;
        set_material_uniforms(cx, vars, &m);
        set_style_uniforms(cx, vars, shape, &assets);
        u4(cx, vars, live_id!(k_state), [self.value, 0.0, 0.0, 0.0]);
        let samples = if self.orbit.is_some() { 1.0 } else { self.samples.round().clamp(1.0, 2.0) };
        u4(cx, vars, live_id!(k_cam), [self.yaw, self.elevation, self.zoom, samples]);
        self.draw_view.draw_abs(cx, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_view.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if fe.tap_count == 2 {
                    self.set_camera(cx, CAMERA_HOME[0], CAMERA_HOME[1], CAMERA_HOME[2]);
                }
                self.orbit = Some((fe.abs, [self.yaw, self.elevation]));
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some((start, cam)) = self.orbit {
                    // The bench's orbit: 3.2 radians across the view's width.
                    let k = 3.2 / fe.rect.size.x.max(1.0);
                    let d = fe.abs - start;
                    self.set_camera(cx, cam[0] + d.x * k, cam[1] + d.y * k, self.zoom);
                }
            }
            Hit::FingerUp(_) => {
                // Back to the full samples once the camera is let go.
                self.orbit = None;
                self.draw_view.redraw(cx);
                cx.set_cursor(MouseCursor::Hand);
            }
            // Ctrl (or Cmd) and the wheel zoom; the wheel alone scrolls the
            // page, so a page scrolled past the view is not caught by it.
            Hit::FingerScroll(fe) if fe.modifiers.control || fe.modifiers.logo => {
                let z = self.zoom * (1.0 - fe.scroll.y * 0.002).clamp(0.5, 2.0);
                self.set_camera(cx, self.yaw, self.elevation, z);
                event.set_scroll_handled(Vec2Index::X);
                event.set_scroll_handled(Vec2Index::Y);
            }
            _ => {}
        }
    }
}

impl KnobView3dRef {
    pub fn set_material(&self, cx: &mut Cx, m: &KnobMaterial) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_material(cx, m);
        }
    }

    pub fn set_shape(&self, cx: &mut Cx, shape: &KnobShape) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_shape(cx, shape);
        }
    }

    pub fn set_style(&self, cx: &mut Cx, style: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_style(cx, style);
        }
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }
}
