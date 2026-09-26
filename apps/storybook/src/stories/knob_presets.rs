//! The Material component's second page: the Material Bench's knob presets.
//!
//! Every one of the bench's nineteen knob styles, live, in the material the
//! controls pick, on that material's ground; the picked style large and in
//! 3D beside them. The knobs are the storybook's port of the bench's knob
//! engine (`crate::knob`); this page is its host: it holds the material the
//! controls write, hands it to every knob and to the 3D view, and picks the
//! style a knob in the gallery is tapped on.
use crate::controls::{ControlValue, StoryControlAction};
use crate::knob::presets::{KnobMaterial, MATERIALS, STYLES};
use crate::knob::widgets::{set_material_uniforms, KnobView3dWidgetExt, TurnedKnobAction, TurnedKnobWidgetExt};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

/// The material the page opens on: the chrome, the bench's showpiece.
const DEFAULT_MATERIAL: usize = 10;
/// And the style: the winged knob.
const DEFAULT_STYLE: usize = 9;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::knob::script_mod(vm);
    page::script_mod(vm)
}

mod page {
    use super::KnobPresets;
    use crate::makepad_widgets::*;

    script_mod! {
        use mod.prelude.widgets.*
        use mod.widgets.*
        use mod.storybook.*

        mod.storybook.KnobPresetsBase = #(KnobPresets::register_widget(vm))
        mod.storybook.KnobPresets = set_type_default() do mod.storybook.KnobPresetsBase{
            width: Fill
            height: Fill
        }

        // One gallery cell: a knob and its style's name under it.
        let KnobCell = View{
            width: 136.
            height: Fit
            flow: Down
            align: Align{x: 0.5 y: 0.0}
            knob := TurnedKnob{width: 136. height: 128. fill: 0.5}
            name := Label{
                text: ""
                draw_text +: {text_style: theme.font_regular{font_size: 9.5}}
            }
        }

        let PageNote = P{
            width: Fill
            text: ""
        }

        mod.stories.MaterialKnobPresets = mod.storybook.KnobPresets{
            flow: Down
            spacing: 14.
            padding: Inset{left: 24. right: 24. top: 20. bottom: 24.}
            scroll_bars: ScrollBars{
                show_scroll_x: false
                show_scroll_y: true
                scroll_bar_y.drag_scrolling: true
            }
            // The page is the material's ground, through the same exposure
            // and roll-off as the knobs, so their quads meet it seamlessly.
            show_bg: true
            draw_bg +: {..mod.storybook.KnobGroundFill}

            intro := PageNote{text: "The Material Bench's knob engine, ported: nineteen knob styles in eleven materials. Every knob here is live -- drag one to turn them all, tap one to pick its style. The large knob and the 3D view show the picked style; drag the 3D view to orbit it, ctrl-scroll to zoom, double-tap to put the camera back."}
            stage := View{
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true}
                spacing: 16.
                align: Align{x: 0.0 y: 0.5}
                view3d := KnobView3d{width: 403. height: 290.}
                side := View{
                    width: 270.
                    height: Fit
                    flow: Down
                    spacing: 4.
                    align: Align{x: 0.5 y: 0.0}
                    big := TurnedKnob{width: 270. height: 270. fill: 0.62}
                    caption := Label{
                        text: ""
                        draw_text +: {text_style: theme.font_bold{font_size: 11}}
                    }
                }
            }
            gallery := View{
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true}
                c0 := KnobCell{}
                c1 := KnobCell{}
                c2 := KnobCell{}
                c3 := KnobCell{}
                c4 := KnobCell{}
                c5 := KnobCell{}
                c6 := KnobCell{}
                c7 := KnobCell{}
                c8 := KnobCell{}
                c9 := KnobCell{}
                c10 := KnobCell{}
                c11 := KnobCell{}
                c12 := KnobCell{}
                c13 := KnobCell{}
                c14 := KnobCell{}
                c15 := KnobCell{}
                c16 := KnobCell{}
                c17 := KnobCell{}
                c18 := KnobCell{}
            }
        }
    }
}

/// The material and knob state the page's controls write, one live
/// property per control. A material preset writes all of them.
#[derive(Script, Widget)]
pub struct KnobPresets {
    #[deref]
    view: View,
    #[live]
    style: f64,
    #[live]
    value: f64,
    #[live]
    lit: bool,
    #[live]
    level: f64,
    #[live]
    lx: f64,
    #[live]
    ly: f64,
    #[live]
    lz: f64,
    #[live]
    li: f64,
    #[live]
    bw: f64,
    #[live]
    bc: f64,
    #[live]
    raise: f64,
    #[live]
    sink: f64,
    #[live]
    spec: f64,
    #[live]
    rough: f64,
    #[live]
    ao: f64,
    #[live]
    rim: f64,
    #[live]
    gloss: f64,
    #[live]
    glow: f64,
    #[live]
    shadow: f64,
    #[live]
    sblur: f64,
    #[live]
    fall: f64,
    #[live]
    oao: f64,
    #[live]
    inner: f64,
    #[live]
    inner_r: f64,
    #[live]
    lip: f64,
    #[live]
    facegrad: f64,
    #[live]
    hair: f64,
    #[live]
    aoreach: f64,
    #[live]
    pdepth: f64,
    #[live]
    psmooth: f64,
    #[live]
    pfin: f64,
    #[live]
    mbev: f64,
    #[live]
    env: f64,
    #[live]
    metal: f64,
    #[live]
    ev: f64,
    #[live]
    roll: f64,
    #[live]
    coat: f64,
    #[live]
    coatr: f64,
    #[live]
    envk: f64,
    #[live]
    persp: f64,
    #[live]
    ground: Vec4f,
    #[live]
    body_ink: Vec4f,
    #[live]
    light_ink: Vec4f,
    #[live]
    shadow_ink: Vec4f,
    #[live]
    glow_ink: Vec4f,
    #[live]
    ptr_ink: Vec4f,
    /// What the children were last handed, so a draw that changes nothing
    /// hands them nothing.
    #[rust]
    pushed: Option<(KnobMaterial, usize, f64, bool)>,
}

fn color_of(c: u32) -> Vec4f {
    let v = crate::knob::bake::ink(c);
    vec4(v[0] as f32, v[1] as f32, v[2] as f32, 1.0)
}

fn ink_of(c: Vec4f) -> u32 {
    let b = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 255;
    (b(c.x) << 24) | (b(c.y) << 16) | (b(c.z) << 8) | 0xFF
}

impl KnobPresets {
    fn load(&mut self, m: &KnobMaterial) {
        self.level = m.level;
        self.lx = m.lx;
        self.ly = m.ly;
        self.lz = m.lz;
        self.li = m.li;
        self.bw = m.bw;
        self.bc = m.bc;
        self.raise = m.raise;
        self.sink = m.sink;
        self.spec = m.spec;
        self.rough = m.rough;
        self.ao = m.ao;
        self.rim = m.rim;
        self.gloss = m.gloss;
        self.glow = m.glow;
        self.shadow = m.shadow;
        self.sblur = m.sblur;
        self.fall = m.fall;
        self.oao = m.oao;
        self.inner = m.inner;
        self.inner_r = m.inner_r;
        self.lip = m.lip;
        self.facegrad = m.facegrad;
        self.hair = m.hair;
        self.aoreach = m.aoreach;
        self.pdepth = m.pdepth;
        self.psmooth = m.psmooth;
        self.pfin = m.pfin;
        self.mbev = m.mbev;
        self.env = m.env;
        self.metal = m.metal;
        self.ev = m.ev;
        self.roll = m.roll;
        self.coat = m.coat;
        self.coatr = m.coatr;
        self.envk = m.envk;
        self.persp = m.persp;
        self.ground = color_of(m.ground);
        self.body_ink = color_of(m.body_ink);
        self.light_ink = color_of(m.light_ink);
        self.shadow_ink = color_of(m.shadow_ink);
        self.glow_ink = color_of(m.glow_ink);
        self.ptr_ink = color_of(m.ptr_ink);
    }

    /// The material the controls describe now.
    fn material(&self) -> KnobMaterial {
        KnobMaterial {
            name: "custom",
            level: self.level,
            lx: self.lx,
            ly: self.ly,
            lz: self.lz,
            li: self.li,
            bw: self.bw,
            bc: self.bc,
            raise: self.raise,
            sink: self.sink,
            spec: self.spec,
            rough: self.rough,
            ao: self.ao,
            rim: self.rim,
            gloss: self.gloss,
            glow: self.glow,
            shadow: self.shadow,
            sblur: self.sblur,
            fall: self.fall,
            oao: self.oao,
            inner: self.inner,
            inner_r: self.inner_r,
            lip: self.lip,
            facegrad: self.facegrad,
            hair: self.hair,
            aoreach: self.aoreach,
            pdepth: self.pdepth,
            psmooth: self.psmooth,
            pfin: self.pfin,
            mbev: self.mbev,
            env: self.env,
            metal: self.metal,
            ev: self.ev,
            roll: self.roll,
            coat: self.coat,
            coatr: self.coatr,
            envk: self.envk,
            persp: self.persp,
            ground: ink_of(self.ground),
            body_ink: ink_of(self.body_ink),
            light_ink: ink_of(self.light_ink),
            shadow_ink: ink_of(self.shadow_ink),
            glow_ink: ink_of(self.glow_ink),
            ptr_ink: ink_of(self.ptr_ink),
        }
    }

    fn style_index(&self) -> usize {
        (self.style.round().max(0.0) as usize).min(STYLES.len() - 1)
    }

    fn cell(i: usize) -> LiveId {
        LiveId::from_str(&format!("c{i}"))
    }

    /// Hand the material, the value and the picked style to every knob and
    /// the 3D view, and colour the text to read on the ground.
    fn push(&mut self, cx: &mut Cx) {
        let m = self.material();
        let style = self.style_index();
        let state = (m, style, self.value, self.lit);
        if self.pushed == Some(state) {
            return;
        }
        let first = self.pushed.is_none();
        let old = self.pushed.replace(state);
        let ground = crate::knob::bake::ink(m.ground);
        let luma = ground[0] * 0.2126 + ground[1] * 0.7152 + ground[2] * 0.0722;
        let text: Vec4f = if luma > 0.45 { vec4(0.14, 0.15, 0.18, 1.0) } else { vec4(0.86, 0.88, 0.91, 1.0) };
        let meta: Vec4f = if luma > 0.45 { vec4(0.30, 0.32, 0.37, 1.0) } else { vec4(0.62, 0.65, 0.70, 1.0) };
        // The picked style's name in the glow ink, where that reads on the
        // ground; a white glow on porcelain does not, and takes the text's.
        let glow = crate::knob::bake::ink(m.glow_ink);
        let glow_luma = glow[0] * 0.2126 + glow[1] * 0.7152 + glow[2] * 0.0722;
        let accent = if (glow_luma - luma).abs() > 0.3 { color_of(m.glow_ink) } else { text };
        let recolour = first || old.map(|o| o.0.ground != m.ground || o.0.glow_ink != m.glow_ink).unwrap_or(true);
        let restyle = first || old.map(|o| o.1 != style).unwrap_or(true);
        for i in 0..STYLES.len() {
            let knob = self.view.turned_knob(cx, &[Self::cell(i), live_id!(knob)]);
            knob.set_style(cx, i);
            knob.set_material(cx, &m);
            knob.set_value(cx, self.value);
            knob.set_lit(cx, self.lit);
            if recolour || restyle {
                let mut name = self.view.widget(cx, &[Self::cell(i), live_id!(name)]);
                let label = STYLES[i].label();
                let color = if i == style { accent } else { meta };
                name.set_text(cx, &label);
                script_apply_eval!(cx, name, { draw_text +: {color: #(color)} });
            }
        }
        let big = self.view.turned_knob(cx, &[live_id!(big)]);
        big.set_style(cx, style);
        big.set_material(cx, &m);
        big.set_value(cx, self.value);
        big.set_lit(cx, self.lit);
        let view3d = self.view.knob_view3d(cx, &[live_id!(view3d)]);
        view3d.set_style(cx, style);
        view3d.set_material(cx, &m);
        view3d.set_value(cx, self.value);
        if recolour || restyle {
            let mut caption = self.view.widget(cx, &[live_id!(caption)]);
            caption.set_text(
                cx,
                &format!(
                    "{} in {}",
                    STYLES[style].label(),
                    MATERIALS
                        .iter()
                        .find(|p| p.ground == m.ground && p.body_ink == m.body_ink)
                        .map(|p| p.name)
                        .unwrap_or("this material")
                ),
            );
            script_apply_eval!(cx, caption, { draw_text +: {color: #(text)} });
            let mut intro = self.view.widget(cx, &[live_id!(intro)]);
            script_apply_eval!(cx, intro, { draw_text +: {color: #(meta)} });
        }
        self.view.redraw(cx);
    }
}

impl ScriptHook for KnobPresets {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.load(&MATERIALS[DEFAULT_MATERIAL]);
        self.style = DEFAULT_STYLE as f64;
        self.value = 0.34;
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        vm.with_cx_mut(|cx| self.view.redraw(cx));
    }
}

impl Widget for KnobPresets {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.push(cx);
        let m = self.material();
        set_material_uniforms(cx, &mut self.view.draw_bg.draw_vars, &m);
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        let mut picked = None;
        let mut turned = None;
        for action in actions.iter() {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            match wa.cast::<TurnedKnobAction>() {
                TurnedKnobAction::Changed(v) => turned = Some(v),
                TurnedKnobAction::Tapped => {
                    for i in 0..STYLES.len() {
                        let knob = self.view.turned_knob(cx, &[Self::cell(i), live_id!(knob)]);
                        if knob.widget_uid() == wa.widget_uid {
                            picked = Some(i);
                        }
                    }
                }
                TurnedKnobAction::None => {}
            }
        }
        // What a knob does on the page, the controls show: the value
        // slider follows a turn and the style's two controls a pick.
        let uid = self.widget_uid();
        if let Some(v) = turned {
            self.value = v;
            self.push(cx);
            cx.widget_action(uid, StoryControlAction::Set { label: VALUE, value: ControlValue::Number(v) });
        }
        if let Some(i) = picked {
            self.style = i as f64;
            self.push(cx);
            cx.widget_action(uid, KnobPresetsAction::StylePicked(i));
            cx.widget_action(uid, StoryControlAction::Set { label: STYLE_PRESET, value: ControlValue::Choice(i) });
            cx.widget_action(
                uid,
                StoryControlAction::Set { label: STYLE_INDEX, value: ControlValue::Number(i as f64) },
            );
        }
        cx.extend_actions(actions);
    }
}

/// What the page raised.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum KnobPresetsAction {
    /// A knob in the gallery was tapped: its style is the picked one now.
    StylePicked(usize),
    #[default]
    None,
}

// ---- the controls ----

const STYLE_PRESET: &str = "Style";
const STYLE_INDEX: &str = "Style index";
const VALUE: &str = "Value";
const LATCHED: &str = "Latched (LEDs lit)";
const MATERIAL: &str = "Material";
const LIGHT_X: &str = "Light x (right +)";
const LIGHT_Y: &str = "Light y (down +)";
const LIGHT_Z: &str = "Light z (toward you)";
const INTENSITY: &str = "Intensity";
const TIER: &str = "Tier (0 / 1 / 2)";
const SPECULAR: &str = "Specular";
const ROUGHNESS: &str = "Roughness";
const METALLIC: &str = "Metallic";
const COAT: &str = "Clear coat";
const COAT_ROUGH: &str = "Coat roughness";
const STUDIO: &str = "Studio (0 / 1 chrome / 2 outdoor)";
const REFLECTION: &str = "Reflection";
const EXPOSURE: &str = "Exposure (EV)";
const ROLL: &str = "Highlight roll-off";
const PERSPECTIVE: &str = "Reflection perspective";
const RAISE: &str = "Raise (pt)";
const SINK: &str = "Sink (pt)";
const RIM: &str = "Rim";
const GLOSS: &str = "Gloss sweep";
const HAIRLINE: &str = "Hairline edge";
const GLOW: &str = "Glow (latched)";
const DEPTH: &str = "Profile depth";
const CREASE: &str = "Min crease blur";
const MARK_FINISH: &str = "Mark finish (paint / engrave / emboss / LED)";
const MARK_BEVEL: &str = "Mark bevel / LED housing";
const BEVEL_WIDTH: &str = "Well bevel width";
const BEVEL_CURVE: &str = "Well bevel curve";
const OCCLUSION: &str = "Well occlusion";
const OCCLUSION_REACH: &str = "Occlusion reach";
const FACE_GRADIENT: &str = "Well face gradient";
const CAST_SHADOW: &str = "Cast shadow";
const SHADOW_BLUR: &str = "Shadow blur (pt)";
const FALLOFF: &str = "Falloff (linear to expo)";
const CONTACT: &str = "Contact occlusion";
const GROUND_LIP: &str = "Ground lip";
const INNER_SHADOW: &str = "Inner shadow (wells)";
const INNER_BLUR: &str = "Inner blur (pt)";
const GROUND: &str = "Ground";
const BODY: &str = "Knob body";
const LIGHT_INK: &str = "Light ink";
const SHADOW_INK: &str = "Shadow ink (multiplies)";
const GLOW_INK: &str = "Glow ink";
const POINTER_INK: &str = "Pointer ink";

const MATERIAL_NAMES: &[&str] = &[
    "Neumorphic",
    "Moulded",
    "Glossy",
    "Milled",
    "Porcelain",
    "Onyx",
    "Gunmetal",
    "Charcoal",
    "Obsidian",
    "Aluminium",
    "Chrome",
];

const STYLE_NAMES: &[&str] = &[
    "Classic",
    "Fluted",
    "Knurled",
    "Pointer",
    "Skirted",
    "Collet",
    "Dome",
    "Chromecap",
    "Lobed",
    "Winged",
    "Dial",
    "Chamfered",
    "Chicken",
    "Dimpled",
    "Fingerdimple",
    "Slotted",
    "Scalloped",
    "Grooved",
    "Cutwing",
];

/// Every material control's value in one preset: the whole material.
fn material_preset(option: usize) -> Vec<(&'static str, ControlValue)> {
    use ControlValue::{Color, Number};
    let Some(m) = MATERIALS.get(option) else {
        return Vec::new();
    };
    vec![
        (LIGHT_X, Number(m.lx)),
        (LIGHT_Y, Number(m.ly)),
        (LIGHT_Z, Number(m.lz)),
        (INTENSITY, Number(m.li)),
        (TIER, Number(m.level)),
        (SPECULAR, Number(m.spec)),
        (ROUGHNESS, Number(m.rough)),
        (METALLIC, Number(m.metal)),
        (COAT, Number(m.coat)),
        (COAT_ROUGH, Number(m.coatr)),
        (STUDIO, Number(m.envk)),
        (REFLECTION, Number(m.env)),
        (EXPOSURE, Number(m.ev)),
        (ROLL, Number(m.roll)),
        (PERSPECTIVE, Number(m.persp)),
        (RAISE, Number(m.raise)),
        (SINK, Number(m.sink)),
        (RIM, Number(m.rim)),
        (GLOSS, Number(m.gloss)),
        (HAIRLINE, Number(m.hair)),
        (GLOW, Number(m.glow)),
        (DEPTH, Number(m.pdepth)),
        (CREASE, Number(m.psmooth)),
        (MARK_FINISH, Number(m.pfin)),
        (MARK_BEVEL, Number(m.mbev)),
        (BEVEL_WIDTH, Number(m.bw)),
        (BEVEL_CURVE, Number(m.bc)),
        (OCCLUSION, Number(m.ao)),
        (OCCLUSION_REACH, Number(m.aoreach)),
        (FACE_GRADIENT, Number(m.facegrad)),
        (CAST_SHADOW, Number(m.shadow)),
        (SHADOW_BLUR, Number(m.sblur)),
        (FALLOFF, Number(m.fall)),
        (CONTACT, Number(m.oao)),
        (GROUND_LIP, Number(m.lip)),
        (INNER_SHADOW, Number(m.inner)),
        (INNER_BLUR, Number(m.inner_r)),
        (GROUND, Color(m.ground)),
        (BODY, Color(m.body_ink)),
        (LIGHT_INK, Color(m.light_ink)),
        (SHADOW_INK, Color(m.shadow_ink)),
        (GLOW_INK, Color(m.glow_ink)),
        (POINTER_INK, Color(m.ptr_ink)),
    ]
}

fn style_preset(option: usize) -> Vec<(&'static str, ControlValue)> {
    vec![(STYLE_INDEX, ControlValue::Number(option as f64))]
}

/// A slider on the page's own property.
const fn number(label: &'static str, prop: &'static str, min: f64, max: f64, step: f64, default: f64) -> Control {
    Control { label, target: "", kind: ControlKind::Number { prop, min, max, step, default } }
}

const fn color(label: &'static str, prop: &'static str, default: u32) -> Control {
    Control { label, target: "", kind: ControlKind::Color { prop, default } }
}

const fn section(label: &'static str, open: bool) -> Control {
    Control { label, target: "", kind: ControlKind::Section { open } }
}

const M: KnobMaterial = MATERIALS[DEFAULT_MATERIAL];

pub const STORIES: &[Story] = &[Story {
    key: "containers/material/knobs",
    category: "Containers",
    component: "Material",
    also: &["TurnedKnob", "KnobView3d"],
    name: "Knob presets",
    dsl: "MaterialKnobPresets",
    added: "2026-09-26",
    tags: &["material", "knob", "presets", "3d", "skeuomorph", "bench"],
    doc: "# Knob presets

The Material Bench's knob engine, ported into the storybook: every one of its nineteen knob styles, live, in any of its eleven materials, on that material's ground. The picked style is shown large and in 3D beside the gallery.

## The engine

`TurnedKnob` draws one knob in its own quad: the ground under it with the knob's cast shadow and contact ring, the wells some styles stand in, the knob's face and its marks -- the pointer, the dial ticks and the value arc. `KnobView3d` ray marches the same solid under an orbiting camera. Both live in the storybook (`apps/storybook/src/knob/`); nothing in the widget library depends on them.

A **style** is geometry: a revolve profile drawn as a Bezier curve, a grip (flutes, knurls, lobes), a wing -- a ridge added along the pointer or two cutters taken away -- a cut (a dimple, a slot, scallops, a ring), a flat, a cap, and the marks. A **material** is light and finish: the key light, the tier, the specular and roughness (GGX), metal, a clear coat, the studio it reflects (a softbox studio, a chrome studio, outdoors), exposure and highlight roll-off, the shadow, and seven inks. The two multiply: any style in any material.

What the bench works out in JavaScript is worked out here in Rust, once per style and light: the curves resampled to 256 taps, the revolve's outline by height for the analytic cast-shadow sweep, the wing as a few knots, and the self-shadow over the disc as a 64 x 64 table. The shader reads them from two small textures.

## Reading a knob

- The **cast shadow** is swept analytically along the light from the solid's outline at each height, so a tall wing throws a long, soft shadow and a low skirt a short, crisp one.
- The **face** is lit from the solid's own height field: five taps give the normal, the curvature widens the highlight where the surface turns inside a pixel, and a crease is reflected from both of its sides with the darker kept, so thin bright lines do not sparkle.
- Reflective materials show the studio in the face; metals show it in their own colour.

## Using it

Drag any knob, the large one included, to turn them all; tap a knob in the gallery to pick its style. On the 3D view a drag orbits the camera, ctrl and the wheel zoom, and a double tap puts the camera back.

The Controls tab has the style and value, a material preset that sets every material control at once, and the bench's material controls in folding groups: Light, Surface, Environment, Relief, Wells, Shadow and Colours.",
    subject: "",
    feature: None,
    controls: &[
        section("Knob", true),
        Control { label: STYLE_PRESET, target: "", kind: ControlKind::Preset { options: STYLE_NAMES, default: DEFAULT_STYLE, values: style_preset } },
        number(STYLE_INDEX, "style", 0., 18., 1., DEFAULT_STYLE as f64),
        number(VALUE, "value", 0., 1., 0.01, 0.34),
        Control { label: LATCHED, target: "", kind: ControlKind::Bool { prop: "lit", default: false } },
        section("Material", true),
        Control { label: MATERIAL, target: "", kind: ControlKind::Preset { options: MATERIAL_NAMES, default: DEFAULT_MATERIAL, values: material_preset } },
        section("Light", false),
        number(LIGHT_X, "lx", -1., 1., 0.01, M.lx),
        number(LIGHT_Y, "ly", -1., 1., 0.01, M.ly),
        number(LIGHT_Z, "lz", 0., 1., 0.01, M.lz),
        number(INTENSITY, "li", 0., 2., 0.05, M.li),
        section("Surface", false),
        number(TIER, "level", 0., 2., 1., M.level),
        number(SPECULAR, "spec", 0., 1., 0.01, M.spec),
        number(ROUGHNESS, "rough", 0., 1., 0.01, M.rough),
        number(METALLIC, "metal", 0., 1., 0.01, M.metal),
        number(COAT, "coat", 0., 1., 0.01, M.coat),
        number(COAT_ROUGH, "coatr", 0., 1., 0.01, M.coatr),
        section("Environment", false),
        number(STUDIO, "envk", 0., 2., 1., M.envk),
        number(REFLECTION, "env", 0., 1., 0.01, M.env),
        number(EXPOSURE, "ev", -2., 2., 0.05, M.ev),
        number(ROLL, "roll", 0., 1., 0.01, M.roll),
        number(PERSPECTIVE, "persp", 0., 1., 0.01, M.persp),
        section("Relief", false),
        number(RAISE, "raise", 0., 24., 0.5, M.raise),
        number(SINK, "sink", 0., 24., 0.5, M.sink),
        number(RIM, "rim", 0., 1., 0.01, M.rim),
        number(GLOSS, "gloss", 0., 1., 0.01, M.gloss),
        number(HAIRLINE, "hair", 0., 1., 0.01, M.hair),
        number(GLOW, "glow", 0., 1., 0.01, M.glow),
        number(DEPTH, "pdepth", 0., 3., 0.01, M.pdepth),
        number(CREASE, "psmooth", 0., 24., 1., M.psmooth),
        number(MARK_FINISH, "pfin", 0., 3., 1., M.pfin),
        number(MARK_BEVEL, "mbev", 0., 6., 0.1, M.mbev),
        section("Wells", false),
        number(BEVEL_WIDTH, "bw", 0., 24., 0.5, M.bw),
        number(BEVEL_CURVE, "bc", 0., 1., 0.05, M.bc),
        number(OCCLUSION, "ao", 0., 1., 0.01, M.ao),
        number(OCCLUSION_REACH, "aoreach", 0.25, 3., 0.05, M.aoreach),
        number(FACE_GRADIENT, "facegrad", 0., 1., 0.01, M.facegrad),
        section("Shadow", false),
        number(CAST_SHADOW, "shadow", 0., 1., 0.01, M.shadow),
        number(SHADOW_BLUR, "sblur", 0.5, 40., 0.5, M.sblur),
        number(FALLOFF, "fall", 0., 1., 0.05, M.fall),
        number(CONTACT, "oao", 0., 1., 0.01, M.oao),
        number(GROUND_LIP, "lip", 0., 1., 0.01, M.lip),
        number(INNER_SHADOW, "inner", 0., 1., 0.01, M.inner),
        number(INNER_BLUR, "inner_r", 0.5, 32., 0.5, M.inner_r),
        section("Colours", false),
        color(GROUND, "ground", M.ground),
        color(BODY, "body_ink", M.body_ink),
        color(LIGHT_INK, "light_ink", M.light_ink),
        color(SHADOW_INK, "shadow_ink", M.shadow_ink),
        color(GLOW_INK, "glow_ink", M.glow_ink),
        color(POINTER_INK, "ptr_ink", M.ptr_ink),
    ],
    on_actions: None,
}];
