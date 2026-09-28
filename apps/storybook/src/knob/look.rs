//! What a knob page's controls write: the style, the value and the whole
//! material, one live property per control, and the controls themselves.
//!
//! Two pages share it, Knob presets and Knob cost. Each holds a `look` and
//! lists the same controls ([`knob_controls!`]), whose properties are
//! `look.*` on the page: a material preset writes every one of them. A page
//! that edits the shape as well hands the macro its own style preset and
//! its shape's controls, which stand between the knob's and the material's.
use crate::controls::ControlValue;
use crate::knob::bake::ink;
use crate::knob::presets::{KnobMaterial, MATERIALS, STYLES};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind};

script_mod! {
    use mod.prelude.widgets_internal.*

    // The look's type default: what lets a control write `look.lx` into a
    // page that holds one.
    mod.storybook.KnobLook = set_type_default() do #(KnobLook::script_component(vm)){}
}

/// The material a page opens on: the charcoal, matte on a warm dark
/// ground, its marks LEDs.
pub const DEFAULT_MATERIAL: usize = 8;
/// And the style: the winged knob.
pub const DEFAULT_STYLE: usize = 8;
/// And the value.
pub const DEFAULT_VALUE: f64 = 0.34;

/// The material and knob state the controls write, one live property per
/// control. A material preset writes all of them.
///
/// A nested live struct is built without its `on_after_new`, so the page
/// that holds one calls [`KnobLook::reset`] from its own.
#[derive(Script, ScriptHook)]
pub struct KnobLook {
    #[live]
    pub style: f64,
    #[live]
    pub value: f64,
    #[live]
    pub lit: bool,
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
    lights: f64,
    #[live]
    panes: f64,
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
}

pub fn color_of(c: u32) -> Vec4f {
    let v = ink(c);
    vec4(v[0] as f32, v[1] as f32, v[2] as f32, 1.0)
}

pub fn ink_of(c: Vec4f) -> u32 {
    let b = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 255;
    (b(c.x) << 24) | (b(c.y) << 16) | (b(c.z) << 8) | 0xFF
}

impl KnobLook {
    /// The look a page opens on: the default material, style and value.
    pub fn reset(&mut self) {
        self.load(&MATERIALS[DEFAULT_MATERIAL]);
        self.style = DEFAULT_STYLE as f64;
        self.value = DEFAULT_VALUE;
    }

    pub fn load(&mut self, m: &KnobMaterial) {
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
        self.lights = m.lights;
        self.panes = m.panes;
        self.ground = color_of(m.ground);
        self.body_ink = color_of(m.body_ink);
        self.light_ink = color_of(m.light_ink);
        self.shadow_ink = color_of(m.shadow_ink);
        self.glow_ink = color_of(m.glow_ink);
        self.ptr_ink = color_of(m.ptr_ink);
    }

    /// The material the controls describe now.
    pub fn material(&self) -> KnobMaterial {
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
            lights: self.lights,
            panes: self.panes,
            ground: ink_of(self.ground),
            body_ink: ink_of(self.body_ink),
            light_ink: ink_of(self.light_ink),
            shadow_ink: ink_of(self.shadow_ink),
            glow_ink: ink_of(self.glow_ink),
            ptr_ink: ink_of(self.ptr_ink),
        }
    }

    pub fn style_index(&self) -> usize {
        (self.style.round().max(0.0) as usize).min(STYLES.len() - 1)
    }

    /// The preset's name when the material is one of them untouched (by its
    /// ground and body), else "this material". Of two with the same inks
    /// (the chrome and the showroom chrome), the one hung with these lights.
    pub fn material_name(&self) -> &'static str {
        let m = self.material();
        let inks = |p: &&KnobMaterial| p.ground == m.ground && p.body_ink == m.body_ink;
        MATERIALS
            .iter()
            .filter(inks)
            .find(|p| p.lights == m.lights)
            .or_else(|| MATERIALS.iter().find(inks))
            .map(|p| p.name)
            .unwrap_or("this material")
    }
}

// ---- the controls ----

pub const STYLE_PRESET: &str = "Style";
pub const STYLE_INDEX: &str = "Style index";
pub const VALUE: &str = "Value";
pub const LATCHED: &str = "Latched (LEDs lit)";
pub const MATERIAL: &str = "Material";
pub const LIGHT_X: &str = "Light x (right +)";
pub const LIGHT_Y: &str = "Light y (down +)";
pub const LIGHT_Z: &str = "Light z (toward you)";
pub const INTENSITY: &str = "Intensity";
pub const TIER: &str = "Tier (0 / 1 / 2)";
pub const SPECULAR: &str = "Specular";
pub const ROUGHNESS: &str = "Roughness";
pub const METALLIC: &str = "Metallic";
pub const COAT: &str = "Clear coat";
pub const COAT_ROUGH: &str = "Coat roughness";
pub const STUDIO: &str = "Studio (0 / 1 chrome / 2 outdoor)";
pub const STUDIO_LIGHTS: &str = "Studio lights";
pub const STUDIO_PANES: &str = "Ceiling panes";
pub const REFLECTION: &str = "Reflection";
pub const EXPOSURE: &str = "Exposure (EV)";
pub const ROLL: &str = "Highlight roll-off";
pub const PERSPECTIVE: &str = "Reflection perspective";
pub const RAISE: &str = "Raise (pt)";
pub const SINK: &str = "Sink (pt)";
pub const RIM: &str = "Rim";
pub const GLOSS: &str = "Gloss sweep";
pub const HAIRLINE: &str = "Hairline edge";
pub const GLOW: &str = "Glow (latched)";
pub const DEPTH: &str = "Profile depth";
pub const CREASE: &str = "Min crease blur";
pub const MARK_FINISH: &str = "Mark finish (paint / engrave / emboss / LED)";
pub const MARK_BEVEL: &str = "Mark bevel / LED housing";
pub const BEVEL_WIDTH: &str = "Well bevel width";
pub const BEVEL_CURVE: &str = "Well bevel curve";
pub const OCCLUSION: &str = "Well occlusion";
pub const OCCLUSION_REACH: &str = "Occlusion reach";
pub const FACE_GRADIENT: &str = "Well face gradient";
pub const CAST_SHADOW: &str = "Cast shadow";
pub const SHADOW_BLUR: &str = "Shadow blur (pt)";
pub const FALLOFF: &str = "Falloff (linear to expo)";
pub const CONTACT: &str = "Contact occlusion";
pub const GROUND_LIP: &str = "Ground lip";
pub const INNER_SHADOW: &str = "Inner shadow (wells)";
pub const INNER_BLUR: &str = "Inner blur (pt)";
pub const GROUND: &str = "Ground";
pub const BODY: &str = "Knob body";
pub const LIGHT_INK: &str = "Light ink";
pub const SHADOW_INK: &str = "Shadow ink (multiplies)";
pub const GLOW_INK: &str = "Glow ink";
pub const POINTER_INK: &str = "Pointer ink";

pub const MATERIAL_NAMES: &[&str] = &[
    "Neumorphic",
    "Neumorphic dark",
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
    "Showroom chrome",
];

pub const STYLE_NAMES: &[&str] = &[
    "Classic",
    "Fluted",
    "Knurled",
    "Pointer",
    "Skirted",
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
pub fn material_preset(option: usize) -> Vec<(&'static str, ControlValue)> {
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
        (STUDIO_LIGHTS, Number(m.lights)),
        (STUDIO_PANES, Number(m.panes)),
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

/// The style's index: what a page that draws the stock styles writes for a
/// style preset.
pub fn style_preset(option: usize) -> Vec<(&'static str, ControlValue)> {
    vec![(STYLE_INDEX, ControlValue::Number(option as f64))]
}

/// A slider on a property of the page.
pub const fn number(label: &'static str, prop: &'static str, min: f64, max: f64, step: f64, default: f64) -> Control {
    Control { label, target: "", kind: ControlKind::Number { prop, min, max, step, default } }
}

pub const fn color(label: &'static str, prop: &'static str, default: u32) -> Control {
    Control { label, target: "", kind: ControlKind::Color { prop, default } }
}

pub const fn section(label: &'static str, open: bool) -> Control {
    Control { label, target: "", kind: ControlKind::Section { open } }
}

/// The material the controls open on.
pub const M: KnobMaterial = MATERIALS[DEFAULT_MATERIAL];

/// The knob controls every knob page lists, on the page's `look`, with the
/// page's own controls after them: the style and value, a material preset
/// that sets every material control at once, and the material controls in
/// folding groups.
///
/// A page that edits the shape names its style preset's values and its
/// shape's controls first: `knob_controls!(style: self::style_preset, shape:
/// [...], extra...)`. The shape's controls stand after the knob's and before
/// the material's. Give the style preset by a path (`self::...`): the list
/// is built where this module's names are in scope, `style_preset` among
/// them.
#[macro_export]
macro_rules! knob_controls {
    (style: $style:expr, shape: [$($shape:expr),* $(,)?] $(, $extra:expr)* $(,)?) => {{
        use $crate::knob::look::*;
        use $crate::registry::{Control, ControlKind};
        &[
            section("Knob", true),
            Control {
                label: STYLE_PRESET,
                target: "",
                kind: ControlKind::Preset { options: STYLE_NAMES, default: DEFAULT_STYLE, values: $style },
            },
            number(STYLE_INDEX, "look.style", 0., ($crate::knob::presets::STYLES.len() - 1) as f64, 1., DEFAULT_STYLE as f64),
            number(VALUE, "look.value", 0., 1., 0.01, DEFAULT_VALUE),
            Control { label: LATCHED, target: "", kind: ControlKind::Bool { prop: "look.lit", default: false } },
            $($shape,)*
            section("Material", true),
            Control {
                label: MATERIAL,
                target: "",
                kind: ControlKind::Preset {
                    options: MATERIAL_NAMES,
                    default: DEFAULT_MATERIAL,
                    values: material_preset,
                },
            },
            section("Light", false),
            number(LIGHT_X, "look.lx", -1., 1., 0.01, M.lx),
            number(LIGHT_Y, "look.ly", -1., 1., 0.01, M.ly),
            number(LIGHT_Z, "look.lz", 0., 1., 0.01, M.lz),
            number(INTENSITY, "look.li", 0., 2., 0.05, M.li),
            section("Surface", false),
            number(TIER, "look.level", 0., 2., 1., M.level),
            number(SPECULAR, "look.spec", 0., 1., 0.01, M.spec),
            number(ROUGHNESS, "look.rough", 0., 1., 0.01, M.rough),
            number(METALLIC, "look.metal", 0., 1., 0.01, M.metal),
            number(COAT, "look.coat", 0., 1., 0.01, M.coat),
            number(COAT_ROUGH, "look.coatr", 0., 1., 0.01, M.coatr),
            section("Environment", false),
            number(STUDIO, "look.envk", 0., 2., 1., M.envk),
            number(STUDIO_LIGHTS, "look.lights", 0., 12., 1., M.lights),
            number(STUDIO_PANES, "look.panes", 1., 6., 1., M.panes),
            number(REFLECTION, "look.env", 0., 1., 0.01, M.env),
            number(EXPOSURE, "look.ev", -2., 2., 0.05, M.ev),
            number(ROLL, "look.roll", 0., 1., 0.01, M.roll),
            number(PERSPECTIVE, "look.persp", 0., 1., 0.01, M.persp),
            section("Relief", false),
            number(RAISE, "look.raise", 0., 24., 0.5, M.raise),
            number(SINK, "look.sink", 0., 24., 0.5, M.sink),
            number(RIM, "look.rim", 0., 1., 0.01, M.rim),
            number(GLOSS, "look.gloss", 0., 1., 0.01, M.gloss),
            number(HAIRLINE, "look.hair", 0., 1., 0.01, M.hair),
            number(GLOW, "look.glow", 0., 1., 0.01, M.glow),
            number(DEPTH, "look.pdepth", 0., 3., 0.01, M.pdepth),
            number(CREASE, "look.psmooth", 0., 24., 1., M.psmooth),
            number(MARK_FINISH, "look.pfin", 0., 3., 1., M.pfin),
            number(MARK_BEVEL, "look.mbev", 0., 6., 0.1, M.mbev),
            section("Wells", false),
            number(BEVEL_WIDTH, "look.bw", 0., 24., 0.5, M.bw),
            number(BEVEL_CURVE, "look.bc", 0., 1., 0.05, M.bc),
            number(OCCLUSION, "look.ao", 0., 1., 0.01, M.ao),
            number(OCCLUSION_REACH, "look.aoreach", 0.25, 3., 0.05, M.aoreach),
            number(FACE_GRADIENT, "look.facegrad", 0., 1., 0.01, M.facegrad),
            section("Shadow", false),
            number(CAST_SHADOW, "look.shadow", 0., 1., 0.01, M.shadow),
            number(SHADOW_BLUR, "look.sblur", 0.5, 40., 0.5, M.sblur),
            number(FALLOFF, "look.fall", 0., 1., 0.05, M.fall),
            number(CONTACT, "look.oao", 0., 1., 0.01, M.oao),
            number(GROUND_LIP, "look.lip", 0., 1., 0.01, M.lip),
            number(INNER_SHADOW, "look.inner", 0., 1., 0.01, M.inner),
            number(INNER_BLUR, "look.inner_r", 0.5, 32., 0.5, M.inner_r),
            section("Colours", false),
            color(GROUND, "look.ground", M.ground),
            color(BODY, "look.body_ink", M.body_ink),
            color(LIGHT_INK, "look.light_ink", M.light_ink),
            color(SHADOW_INK, "look.shadow_ink", M.shadow_ink),
            color(GLOW_INK, "look.glow_ink", M.glow_ink),
            color(POINTER_INK, "look.ptr_ink", M.ptr_ink),
            $($extra),*
        ]
    }};
    ($($extra:expr),* $(,)?) => {
        $crate::knob_controls!(style: $crate::knob::look::style_preset, shape: [] $(, $extra)*)
    };
}
