//! The knob engine's widgets: `TurnedKnob`, one knob of a style in a
//! material in its own quad, turned by a drag; `KnobView3d`, the same solid
//! ray marched under an orbiting camera; and the bake cache they share.
use super::bake::{self, GeomBake, GeomConsts, GeomKey, ShadeConsts, ShadeKey, DATA_ROWS, KNOT_FLOATS, SHADE_N, TAPS};
use super::presets::{KnobMaterial, KnobStyle, MATERIALS, STYLES};
use super::shader::{DrawKnobView3d, DrawTurnedKnob};
use crate::makepad_widgets::*;
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.storybook.TurnedKnobBase = #(TurnedKnob::register_widget(vm))
    /** One knob of the bench's knob engine: `style` (0..18) in `material`
     * (0..10), turned to `value`. A drag turns it; a tap raises `Tapped`. */
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

/// A style's geometry bake as the GPU holds it.
pub struct GeomAssets {
    pub key: GeomKey,
    pub data: Texture,
    pub consts: GeomConsts,
    bake: GeomBake,
}

/// A style's shadow bake as the GPU holds it.
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
    /// Whether these are the bakes for this style in this material.
    pub fn fits(&self, style: usize, m: &KnobMaterial) -> bool {
        self.shade.key == ShadeKey::new(style, m)
    }
}

/// The bakes in use, newest last. A style's curves are baked once per crease
/// blur and its shadows once per light, and every knob and view that draws
/// it shares them: moving the light re-bakes the shadows only.
#[derive(Default)]
struct BakeCache {
    geoms: Vec<Rc<GeomAssets>>,
    shades: Vec<Rc<ShadeAssets>>,
}

/// How many of each the cache keeps: every style under two lights, so a
/// gallery survives a light being moved back and forth.
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

/// The bakes for this style in this material, from the cache or made now.
pub fn knob_assets(cx: &mut Cx, style: usize, m: &KnobMaterial) -> KnobAssets {
    let style = style.min(STYLES.len() - 1);
    let gkey = GeomKey::new(style, m);
    let skey = ShadeKey::new(style, m);
    let geom = match cached(&mut cx.global::<BakeCache>().geoms, &gkey, |a| &a.key) {
        Some(g) => g,
        None => {
            let bake = bake::bake_geometry(&STYLES[style], &gkey);
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
            let bake = bake::bake_shade(&STYLES[style], &geom.bake, &skey);
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

/// The style's uniforms and its bake's textures.
pub fn set_style_uniforms(cx: &Cx, vars: &mut DrawVars, s: &KnobStyle, a: &KnobAssets) {
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
    /// The style, 0..18, in the order of `STYLES`.
    #[live]
    pub style: f64,
    /// The material, 0..10, in the order of `MATERIALS`, unless the host
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
    assets: Option<KnobAssets>,
    #[rust]
    drag: Option<Drag>,
}

impl TurnedKnob {
    pub fn material(&self) -> KnobMaterial {
        self.custom.unwrap_or(MATERIALS[(self.material.round().max(0.0) as usize).min(MATERIALS.len() - 1)])
    }

    pub fn style_index(&self) -> usize {
        (self.style.round().max(0.0) as usize).min(STYLES.len() - 1)
    }

    pub fn set_material(&mut self, cx: &mut Cx, m: &KnobMaterial) {
        if self.custom.as_ref() != Some(m) {
            self.custom = Some(*m);
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
}

impl Widget for TurnedKnob {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let m = self.material();
        let style = self.style_index();
        let assets = match &self.assets {
            Some(a) if a.fits(style, &m) => a.clone(),
            _ => {
                let a = knob_assets(cx, style, &m);
                self.assets = Some(a.clone());
                a
            }
        };
        let radius = self.fill * rect.size.x.min(rect.size.y) * 0.5;
        let vars = &mut self.draw_knob.draw_vars;
        set_material_uniforms(cx, vars, &m);
        set_style_uniforms(cx, vars, &STYLES[style], &assets);
        u4(cx, vars, live_id!(k_state), [self.value, if self.lit { 1.0 } else { 0.0 }, self.edge_fade, 0.0]);
        u4(cx, vars, live_id!(k_geom), [rect.size.x * 0.5, rect.size.y * 0.5, radius.max(1.0), self.unit_radius]);
        self.draw_knob.draw_abs(cx, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_knob.area()) {
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

    pub fn set_material(&mut self, cx: &mut Cx, m: &KnobMaterial) {
        if self.custom.as_ref() != Some(m) {
            self.custom = Some(*m);
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
        let style = self.style_index();
        let assets = match &self.assets {
            Some(a) if a.fits(style, &m) => a.clone(),
            _ => {
                let a = knob_assets(cx, style, &m);
                self.assets = Some(a.clone());
                a
            }
        };
        let vars = &mut self.draw_view.draw_vars;
        set_material_uniforms(cx, vars, &m);
        set_style_uniforms(cx, vars, &STYLES[style], &assets);
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
