//! The surface material story: the moulded relief the base shaders carry,
//! switched on here whatever theme the catalogue is showing, with the
//! material bench's controls to tune it live.
use crate::controls::ControlValue;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // The material switched on and given every number it has, whatever
    // theme the catalogue is showing: the material bench's neumorphic
    // preset, at tier 2 so the rim, the gloss and the specular have
    // something to show. Every stock theme carries the material's defaults
    // at material_level 0, and every material sheet its own numbers, so a
    // page that let the theme supply any of them would start somewhere
    // other than where the controls panel says it is. The panel writes the
    // same uniforms on every control below, one packed vector at a time.
    let neu_tier = 2.0
    // light x, y, z, intensity
    let neu_light = vec4(-0.35, -0.55, 0.66, 0.70)
    // bevel width, bevel curve, raise, specular
    let neu_relief = vec4(4.0, 0.70, 4.0, 0.0)
    // occlusion, rim, gloss, roughness
    let neu_finish = vec4(0.50, 0.40, 0.0, 0.85)
    // face gradient, hairline, occlusion reach, sink
    let neu_tune = vec4(0.45, 0.0, 1.20, 4.0)
    // cast shadow, blur, falloff, contact occlusion
    let neu_shadow = vec4(0.85, 12.0, 1.0, 0.50)
    // inner shadow, inner blur, ground lip, glow
    let neu_inner = vec4(0.55, 10.0, 0.70, 0.0)
    let neu_light_ink = #xFFFFFFFF
    let neu_shadow_ink = #x9299B3FF
    let neu_glow_ink = #x7C4DFFFF

    // The margin is the room the shadow falls into, paid for again in the
    // padding, since a control keeps its quad and insets its face rather
    // than growing past its clip.
    let Cap = Button{
        padding: Inset{left: 22. right: 22. top: 16. bottom: 16.}
        draw_bg +: {
            material: neu_tier
            material_margin: 10.0
            material_light: neu_light
            material_relief: neu_relief
            material_finish: neu_finish
            material_tune: neu_tune
            material_shadow: neu_shadow
            material_inner: neu_inner
            material_light_ink: neu_light_ink
            material_shadow_ink: neu_shadow_ink
            material_glow_ink: neu_glow_ink
            border_size: 0.0
        }
        draw_text +: {material_ink_glow: 0.0}
    }
    // The three press idioms, one signed number each. The illuminating cap
    // keeps a glow of its own, which the panel leaves alone.
    let Invert = Cap{draw_bg +: {material_press: -9.0 material_press_invert: 1.0}}
    let Deepen = Cap{draw_bg +: {material_press: -2.0 material_press_invert: 0.0}}
    let Illuminate = Cap{
        draw_bg +: {material_press: -1.0 material_press_invert: 0.0 material_inner: vec4(0.55, 10.0, 0.70, 0.6)}
        draw_text +: {material_ink_glow: 0.85}
    }

    let Well = CheckBox{
        draw_bg +: {
            material: neu_tier
            material_light: neu_light
            material_relief: neu_relief
            material_finish: neu_finish
            material_tune: neu_tune
            material_shadow: neu_shadow
            material_inner: neu_inner
            material_light_ink: neu_light_ink
            material_shadow_ink: neu_shadow_ink
            material_glow_ink: neu_glow_ink
            border_size: 0.0
        }
    }
    let Pill = Toggle{
        draw_bg +: {
            material: neu_tier
            material_light: neu_light
            material_relief: neu_relief
            material_finish: neu_finish
            material_tune: neu_tune
            material_shadow: neu_shadow
            material_inner: neu_inner
            material_light_ink: neu_light_ink
            material_shadow_ink: neu_shadow_ink
            material_glow_ink: neu_glow_ink
            border_size: 0.0
        }
    }
    let Groove = Slider{
        width: Fill
        draw_bg +: {
            material: neu_tier
            material_light: neu_light
            material_relief: neu_relief
            material_finish: neu_finish
            material_tune: neu_tune
            material_shadow: neu_shadow
            material_inner: neu_inner
            material_light_ink: neu_light_ink
            material_shadow_ink: neu_shadow_ink
            border_size: 0.0
        }
    }
    let Knob = RotaryKnob{
        text: ""
        draw_bg +: {
            material: neu_tier
            material_margin: 8.0
            material_light: neu_light
            material_relief: neu_relief
            material_finish: neu_finish
            material_tune: neu_tune
            material_shadow: neu_shadow
            material_inner: neu_inner
            material_light_ink: neu_light_ink
            material_shadow_ink: neu_shadow_ink
            material_glow_ink: neu_glow_ink
            border_size: 0.0
        }
    }

    mod.stories.SurfaceMaterialOverview = StoryPage{
        StoryNote{text: "The base shaders carry a moulded material behind one uniform. At 0, every stock theme, they draw what they always drew; at 1 a face is lit from its own distance field (a shoulder, a cast shadow, a well's inner shadow); at 2 it also takes a rim, a gloss sweep and a specular. This page raises the tier on each control, whatever theme is showing, so the relief can be read against the flat page around it."}
        StoryNote{text: "The Controls tab carries the material bench's controls, in its groups: Light, Relief, Surface, Finish, Shadow and Colours, each folded away or back by a click on its heading, and a preset that sets them all to one of the bench's eleven materials. They write every control on this page at once. The page starts on the bench's neumorphic preset at tier 2; its inks were picked for a pale ground, so the Light theme or the Neumorphic sheet shows it as it was tuned."}

        StoryHeading{text: "Raised and sunken"}
        StoryNote{text: "The same material lit from opposite sides. A rounded view stands off the page: its face is lit and its cast shadow, contact ring and light-side lip go UNDER the face, inside the margin border_inset reclaims. A panel is a shallow step in the housing; sunken, it takes the surround's shadow across its face."}
        StoryRow{
            spacing: theme.space_3
            raised := RoundedView{
                width: 160. height: 90.
                draw_bg +: {
                    color: theme.color_outset
                    border_radius: 8.
                    border_inset: vec4(12. 12. 12. 12.)
                    material: neu_tier
                    material_light: neu_light
                    material_relief: neu_relief
                    material_finish: neu_finish
                    material_tune: neu_tune
                    material_shadow: neu_shadow
                    material_inner: neu_inner
                    material_light_ink: neu_light_ink
                    material_shadow_ink: neu_shadow_ink
                }
            }
            panel := PanelView{
                width: 160. height: 90.
                draw_bg +: {
                    material: neu_tier
                    material_light: neu_light
                    material_relief: neu_relief
                    material_finish: neu_finish
                    material_tune: neu_tune
                    material_shadow: neu_shadow
                    material_inner: neu_inner
                    material_light_ink: neu_light_ink
                    material_shadow_ink: neu_shadow_ink
                }
            }
            inset := InsetPanelView{
                width: 160. height: 90.
                draw_bg +: {
                    material: neu_tier
                    material_light: neu_light
                    material_relief: neu_relief
                    material_finish: neu_finish
                    material_tune: neu_tune
                    material_shadow: neu_shadow
                    material_inner: neu_inner
                    material_light_ink: neu_light_ink
                    material_shadow_ink: neu_shadow_ink
                }
            }
        }

        StoryHeading{text: "A button that really goes down"}
        StoryNote{text: "Rest, hover, pressed, focused, disabled. The pointer lifts a cap a quarter more; a press adds material_press, and its sign and size pick the idiom. Disabled moulds the cap flat into the page, which also takes its shadow away. The focus ring stays what it was: focus is a ring, never a change of relief."}
        StoryNote{text: "INVERT: past minus the raise the face crosses zero, the lit and shaded shoulders swap and the cap reads as pushed into the page. material_press_invert dishes the face as well. The neumorphic sheet, and the row the panel's press controls move."}
        StoryRow{
            subject := Invert{text: "Rest"}
            invert_hover := Invert{text: "Hover" animator +: {hover: {default: @on}}}
            invert_pressed := Invert{text: "Pressed" animator +: {hover: {default: @down}}}
            invert_focused := Invert{text: "Focused" animator +: {focus: {default: @on}}}
            invert_disabled := Invert{text: "Disabled" animator +: {disabled: {default: @on}}}
        }
        StoryNote{text: "DEEPEN: the cap drops but stays above the page, its face still convex, so the shadow closes up and the specular dims. Moulded plastic flexes; it does not turn inside out. The molded sheet."}
        StoryRow{
            deepen_rest := Deepen{text: "Rest"}
            deepen_hover := Deepen{text: "Hover" animator +: {hover: {default: @on}}}
            deepen_pressed := Deepen{text: "Pressed" animator +: {hover: {default: @down}}}
            deepen_focused := Deepen{text: "Focused" animator +: {focus: {default: @on}}}
            deepen_disabled := Deepen{text: "Disabled" animator +: {disabled: {default: @on}}}
        }
        StoryNote{text: "ILLUMINATE: the cap barely moves and the glow carries the state -- the face lifts toward the glow ink, a halo spills onto the page, and the label lights with it, pushed past full brightness so it clips bright rather than tints. The glossy and milled sheets."}
        StoryRow{
            illuminate_rest := Illuminate{text: "Rest"}
            illuminate_hover := Illuminate{text: "Hover" animator +: {hover: {default: @on}}}
            illuminate_pressed := Illuminate{text: "Pressed" animator +: {hover: {default: @down}}}
            illuminate_focused := Illuminate{text: "Focused" animator +: {focus: {default: @on}}}
            illuminate_disabled := Illuminate{text: "Disabled" animator +: {disabled: {default: @on}}}
        }

        StoryHeading{text: "Wells and caps"}
        StoryNote{text: "A check box is a well cut into the housing. A toggle is a sunken track with one solid knob travelling in it, the same substance as the housing, throwing its shadow on the track and taking the active ink as it turns on. A slider's track is a groove and its handle a cap standing in it."}
        StoryRow{
            spacing: theme.space_3
            well_off := Well{text: "Off"}
            well_on := Well{text: "On" active: true}
            pill_off := Pill{text: "Off"}
            pill_on := Pill{text: "On" active: true}
        }
        StoryRow{
            groove := Groove{text: "Level"}
        }
        StoryNote{text: "The knob is a domed cap: the shoulder rolls into a shallow dome so the whole face catches the light, and it keeps back from its box by the margin its shadow falls into, held to a third of the radius so a 24-square knob keeps a face. At 24, 32, 44 and 64."}
        StoryRow{
            spacing: theme.space_3
            align: Align{x: 0. y: 1.}
            knob_24 := Knob{width: 24. height: 24.}
            knob_32 := Knob{width: 32. height: 32.}
            knob_44 := Knob{width: 44. height: 44.}
            knob_64 := Knob{width: 64. height: 64.}
        }
    }
}

// The widgets the bench's controls write, by the uniforms each one has. A
// rounded view, a panel and the slider's groove have no glow ink; only a
// button has a press and a lit label; the illuminating row keeps its own
// glow, and the deepening and illuminating rows their own press, so the
// three idioms stay apart however the rest is tuned.
macro_rules! views {
    () => {
        "raised panel inset"
    };
}
macro_rules! invert_row {
    () => {
        "subject invert_hover invert_pressed invert_focused invert_disabled"
    };
}
macro_rules! deepen_row {
    () => {
        "deepen_rest deepen_hover deepen_pressed deepen_focused deepen_disabled"
    };
}
macro_rules! illuminate_row {
    () => {
        "illuminate_rest illuminate_hover illuminate_pressed illuminate_focused illuminate_disabled"
    };
}
macro_rules! wells_and_knobs {
    () => {
        "well_off well_on pill_off pill_on knob_24 knob_32 knob_44 knob_64"
    };
}

/// Every material control on the page.
const EVERY: &str =
    concat!(views!(), " ", invert_row!(), " ", deepen_row!(), " ", illuminate_row!(), " ", wells_and_knobs!(), " groove");
/// Every one but the illuminating row, which keeps its glow.
const INNER: &str = concat!(views!(), " ", invert_row!(), " ", deepen_row!(), " ", wells_and_knobs!(), " groove");
/// Every one with a glow ink.
const GLOW_INK: &str = concat!(invert_row!(), " ", deepen_row!(), " ", illuminate_row!(), " ", wells_and_knobs!());
/// The inverting row, the neumorphic idiom the press controls move.
const PRESSED: &str = invert_row!();
/// The buttons whose label does not already glow.
const LIT_LABEL: &str = concat!(invert_row!(), " ", deepen_row!());

// The bench's labels, shared by the controls and the presets that set them.
const TIER: &str = "Tier (0 / 1 / 2)";
const LIGHT_X: &str = "Light x (right +)";
const LIGHT_Y: &str = "Light y (down +)";
const LIGHT_Z: &str = "Light z (toward you)";
const INTENSITY: &str = "Intensity";
const FACE_GRADIENT: &str = "Face gradient";
const HAIRLINE: &str = "Hairline edge";
const BEVEL_WIDTH: &str = "Bevel width (pt)";
const PROFILE_CURVE: &str = "Profile curve";
const RAISE: &str = "Raise (pt)";
const SINK: &str = "Sink (pt)";
const PRESS_DEPTH: &str = "Press depth (pt)";
const PRESS_DISHES: &str = "Press dishes the face";
const ROUGHNESS: &str = "Roughness";
const SPECULAR: &str = "Specular";
const OCCLUSION: &str = "Occlusion (sunken)";
const OCCLUSION_REACH: &str = "Occlusion reach (x bevel)";
const RIM: &str = "Rim";
const GLOSS: &str = "Gloss sweep";
const GLOW: &str = "Glow";
const INK_GLOW: &str = "Ink glow (latched)";
const CAST_SHADOW: &str = "Cast shadow";
const SHADOW_BLUR: &str = "Shadow blur (pt)";
const FALLOFF: &str = "Falloff (linear to expo)";
const CONTACT: &str = "Contact occlusion";
const INNER_SHADOW: &str = "Inner shadow (sunken)";
const INNER_BLUR: &str = "Inner blur (pt)";
const GROUND_LIP: &str = "Ground lip (extruded)";
const LIGHT_INK: &str = "Light ink";
const SHADOW_INK: &str = "Shadow ink";
const GLOW_INK_LABEL: &str = "Glow ink";

/// One of the bench's materials, the part of it this page has a uniform for.
#[derive(Clone, Copy)]
struct Material {
    level: f64,
    lx: f64,
    ly: f64,
    lz: f64,
    li: f64,
    bw: f64,
    bc: f64,
    raise: f64,
    sink: f64,
    press: f64,
    pinvert: f64,
    spec: f64,
    rough: f64,
    ao: f64,
    aoreach: f64,
    rim: f64,
    gloss: f64,
    glow: f64,
    inkglow: f64,
    facegrad: f64,
    hair: f64,
    shadow: f64,
    sblur: f64,
    fall: f64,
    oao: f64,
    inner: f64,
    inner_r: f64,
    lip: f64,
    light_ink: u32,
    shadow_ink: u32,
    glow_ink: u32,
}

// The bench's four base materials, and the seven it derives from them by
// changing a handful of numbers each, as the bench writes them. Its shadow
// ink is written here as picked, before the bench divides it by its ground
// and rescales the occlusion to match: that step follows the bench's own
// shader, which multiplies by the shadow ink, and the material library here
// mixes toward it.
const NEUMORPHIC: Material = Material {
    level: 1.0,
    lx: -0.35,
    ly: -0.55,
    lz: 0.66,
    li: 0.70,
    bw: 4.0,
    bc: 0.70,
    raise: 4.0,
    sink: 4.0,
    press: -9.0,
    pinvert: 1.0,
    spec: 0.0,
    rough: 0.85,
    ao: 0.50,
    aoreach: 1.20,
    rim: 0.40,
    gloss: 0.0,
    glow: 0.0,
    inkglow: 0.0,
    facegrad: 0.45,
    hair: 0.0,
    shadow: 0.85,
    sblur: 12.0,
    fall: 1.0,
    oao: 0.5,
    inner: 0.55,
    inner_r: 10.0,
    lip: 0.70,
    light_ink: 0xFFFFFFFF,
    shadow_ink: 0x9299B3FF,
    glow_ink: 0x7C4DFFFF,
};
const MOULDED: Material = Material {
    level: 2.0,
    lx: -0.30,
    ly: -0.62,
    lz: 0.72,
    li: 0.85,
    bw: 3.5,
    bc: 0.85,
    raise: 3.0,
    sink: 3.0,
    press: -2.0,
    pinvert: 0.0,
    spec: 0.12,
    rough: 0.60,
    ao: 0.42,
    aoreach: 1.00,
    rim: 0.55,
    gloss: 0.10,
    glow: 0.0,
    inkglow: 0.0,
    facegrad: 0.42,
    hair: 0.30,
    shadow: 0.60,
    sblur: 12.0,
    fall: 1.0,
    oao: 0.35,
    inner: 0.50,
    inner_r: 7.0,
    lip: 0.0,
    light_ink: 0xF6F6F3FF,
    shadow_ink: 0x6B7176FF,
    glow_ink: 0xD24B3AFF,
};
const GLOSSY: Material = Material {
    level: 2.0,
    lx: -0.20,
    ly: -0.70,
    lz: 0.68,
    li: 1.15,
    bw: 2.5,
    bc: 0.70,
    raise: 2.5,
    sink: 3.0,
    press: -1.0,
    pinvert: 0.0,
    spec: 0.35,
    rough: 0.16,
    ao: 0.55,
    aoreach: 1.10,
    rim: 0.62,
    gloss: 0.42,
    glow: 0.55,
    inkglow: 0.85,
    facegrad: 0.80,
    hair: 0.50,
    shadow: 0.70,
    sblur: 10.0,
    fall: 1.0,
    oao: 0.50,
    inner: 0.72,
    inner_r: 9.0,
    lip: 0.0,
    light_ink: 0xFFFFFFFF,
    shadow_ink: 0x05070AFF,
    glow_ink: 0x4DD0E1FF,
};
const MILLED: Material = Material {
    level: 2.0,
    lx: -0.28,
    ly: -0.60,
    lz: 0.74,
    li: 0.85,
    bw: 2.0,
    bc: 0.95,
    raise: 2.0,
    sink: 2.5,
    press: -0.5,
    pinvert: 0.0,
    spec: 0.30,
    rough: 0.34,
    ao: 0.62,
    aoreach: 0.90,
    rim: 0.48,
    gloss: 0.06,
    glow: 0.85,
    inkglow: 0.90,
    facegrad: 0.55,
    hair: 0.70,
    shadow: 0.60,
    sblur: 7.0,
    fall: 1.0,
    oao: 0.55,
    inner: 0.66,
    inner_r: 6.0,
    lip: 0.0,
    light_ink: 0x9AA3ADFF,
    shadow_ink: 0x000000FF,
    glow_ink: 0xFF7A18FF,
};
const PORCELAIN: Material = Material {
    level: 2.0,
    lx: -0.36,
    ly: -0.6,
    lz: 0.72,
    li: 0.95,
    bw: 4.0,
    bc: 0.9,
    raise: 3.0,
    sink: 2.0,
    press: -1.5,
    spec: 0.3,
    rough: 0.22,
    ao: 0.28,
    rim: 0.35,
    gloss: 0.36,
    glow: 0.35,
    shadow: 0.45,
    sblur: 18.0,
    fall: 0.8,
    oao: 0.25,
    inner: 0.35,
    inner_r: 10.0,
    lip: 0.0,
    inkglow: 0.0,
    facegrad: 0.3,
    hair: 0.15,
    aoreach: 1.2,
    pinvert: 0.0,
    light_ink: 0xFFFFFFFF,
    shadow_ink: 0x8E98ADFF,
    glow_ink: 0xFFFFFFFF,
};
const ONYX: Material = Material {
    light_ink: 0xFFF6EAFF,
    shadow_ink: 0x8E98ADFF,
    glow_ink: 0xFF7F1AFF,
    inkglow: 0.9,
    lx: -0.22,
    ly: -0.72,
    lz: 0.62,
    li: 1.25,
    bw: 2.5,
    bc: 0.75,
    raise: 3.0,
    press: -1.5,
    spec: 0.58,
    rough: 0.08,
    gloss: 0.62,
    rim: 0.7,
    glow: 0.75,
    ao: 0.62,
    aoreach: 1.1,
    facegrad: 0.85,
    hair: 0.35,
    shadow: 0.55,
    sblur: 16.0,
    fall: 0.9,
    oao: 0.82,
    inner: 0.8,
    inner_r: 10.0,
    ..GLOSSY
};
const GUNMETAL: Material = Material {
    lx: -0.24,
    ly: -0.68,
    lz: 0.69,
    li: 0.95,
    bw: 2.5,
    raise: 2.5,
    sink: 3.5,
    press: -1.0,
    spec: 0.36,
    rough: 0.3,
    ao: 0.68,
    rim: 0.66,
    gloss: 0.22,
    glow: 0.6,
    shadow: 0.55,
    sblur: 6.0,
    oao: 0.62,
    inner: 0.74,
    inner_r: 5.0,
    facegrad: 0.62,
    hair: 0.85,
    aoreach: 0.85,
    inkglow: 0.15,
    light_ink: 0xCFD5DCFF,
    shadow_ink: 0x07090CFF,
    glow_ink: 0xFF3B30FF,
    ..MILLED
};
const CHARCOAL: Material = Material {
    light_ink: 0xF0E9E0FF,
    shadow_ink: 0x0B0908FF,
    glow_ink: 0x5CC8FFFF,
    inkglow: 0.35,
    lx: -0.22,
    ly: -0.72,
    lz: 0.66,
    li: 0.9,
    bw: 3.0,
    bc: 0.75,
    raise: 2.5,
    sink: 2.5,
    press: -1.5,
    spec: 0.1,
    rough: 0.8,
    gloss: 0.0,
    rim: 0.32,
    glow: 0.62,
    facegrad: 0.3,
    hair: 0.3,
    ao: 0.5,
    aoreach: 1.0,
    shadow: 0.55,
    sblur: 10.0,
    fall: 1.0,
    oao: 0.45,
    inner: 0.6,
    inner_r: 8.0,
    ..GLOSSY
};
const OBSIDIAN: Material = Material {
    lx: -0.3,
    ly: -0.58,
    lz: 0.78,
    li: 0.65,
    bw: 2.0,
    bc: 0.9,
    raise: 1.5,
    sink: 2.0,
    press: -0.5,
    spec: 0.08,
    rough: 0.82,
    ao: 0.55,
    rim: 0.3,
    gloss: 0.0,
    glow: 0.55,
    shadow: 0.45,
    sblur: 10.0,
    fall: 1.0,
    oao: 0.45,
    inner: 0.62,
    inner_r: 6.0,
    inkglow: 0.4,
    facegrad: 0.22,
    hair: 0.22,
    aoreach: 1.0,
    light_ink: 0x7C838CFF,
    shadow_ink: 0x020305FF,
    glow_ink: 0x38D1E6FF,
    ..MILLED
};
const ALUMINIUM: Material = Material {
    li: 1.0,
    bw: 2.5,
    bc: 0.9,
    spec: 0.45,
    rough: 0.38,
    ao: 0.7,
    rim: 0.58,
    gloss: 0.2,
    glow: 0.9,
    shadow: 0.55,
    sblur: 9.0,
    oao: 0.55,
    inner: 0.72,
    inner_r: 5.0,
    facegrad: 0.6,
    hair: 0.65,
    aoreach: 0.9,
    inkglow: 0.85,
    light_ink: 0xF7F8FAFF,
    shadow_ink: 0x3A3D42FF,
    glow_ink: 0xFFFFFFFF,
    ..MILLED
};
// A mirror metal on a dark panel. Its metal, reflection, environment and
// highlight roll-off have no uniform here; what is left is a hard, bright
// specular with no rim, gloss sweep or face gradient.
const CHROME: Material = Material {
    light_ink: 0xFFFFFFFF,
    shadow_ink: 0x040506FF,
    glow_ink: 0x7FD4FFFF,
    level: 2.0,
    spec: 1.0,
    rough: 0.06,
    rim: 0.0,
    gloss: 0.0,
    facegrad: 0.0,
    inkglow: 0.6,
    glow: 0.4,
    ..GLOSSY
};

const PRESET_NAMES: &[&str] = &[
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
const PRESETS: [Material; 11] =
    [NEUMORPHIC, MOULDED, GLOSSY, MILLED, PORCELAIN, ONYX, GUNMETAL, CHARCOAL, OBSIDIAN, ALUMINIUM, CHROME];

/// The values a preset gives the bench's controls. The bench's rule for
/// every material is kept: a held button goes below its surround, at least
/// six tenths of the sink under, and its face dishes by at least 0.8 so it
/// reads as a well rather than as a cap that descended.
fn material_preset(option: usize) -> Vec<(&'static str, ControlValue)> {
    use ControlValue::{Color, Number};
    let Some(m) = PRESETS.get(option).copied() else {
        return Vec::new();
    };
    let press = m.press.min(-(m.raise + 0.6 * m.sink));
    let pinvert = m.pinvert.max(0.8);
    vec![
        (LIGHT_X, Number(m.lx)),
        (LIGHT_Y, Number(m.ly)),
        (LIGHT_Z, Number(m.lz)),
        (INTENSITY, Number(m.li)),
        (TIER, Number(m.level)),
        (FACE_GRADIENT, Number(m.facegrad)),
        (HAIRLINE, Number(m.hair)),
        (BEVEL_WIDTH, Number(m.bw)),
        (PROFILE_CURVE, Number(m.bc)),
        (RAISE, Number(m.raise)),
        (SINK, Number(m.sink)),
        (PRESS_DEPTH, Number(press)),
        (PRESS_DISHES, Number(pinvert)),
        (ROUGHNESS, Number(m.rough)),
        (SPECULAR, Number(m.spec)),
        (OCCLUSION, Number(m.ao)),
        (OCCLUSION_REACH, Number(m.aoreach)),
        (RIM, Number(m.rim)),
        (GLOSS, Number(m.gloss)),
        (GLOW, Number(m.glow)),
        (INK_GLOW, Number(m.inkglow)),
        (CAST_SHADOW, Number(m.shadow)),
        (SHADOW_BLUR, Number(m.sblur)),
        (FALLOFF, Number(m.fall)),
        (CONTACT, Number(m.oao)),
        (INNER_SHADOW, Number(m.inner)),
        (INNER_BLUR, Number(m.inner_r)),
        (GROUND_LIP, Number(m.lip)),
        (LIGHT_INK, Color(m.light_ink)),
        (SHADOW_INK, Color(m.shadow_ink)),
        (GLOW_INK_LABEL, Color(m.glow_ink)),
    ]
}

/// A bench slider: a number on every control in `target`.
const fn number(label: &'static str, target: &'static str, prop: &'static str, min: f64, max: f64, step: f64, default: f64) -> Control {
    Control { label, target, kind: ControlKind::Number { prop, min, max, step, default } }
}

const fn section(label: &'static str) -> Control {
    Control { label, target: "", kind: ControlKind::Section { open: true } }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/material/overview",
    category: "Containers",
    component: "Material",
    also: &["RoundedView", "PanelView", "InsetPanelView", "Button", "CheckBox", "Toggle", "Slider", "RotaryKnob"],
    name: "Overview",
    dsl: "SurfaceMaterialOverview",
    added: "2026-09-24",
    tags: &["material", "skeuomorph", "neumorphic"],
    doc: "# Surface material

The base shaders of the rounded view, the panel, the button, the check box, the toggle, the slider and the knob carry a moulded material behind one uniform, `material`. It defaults to the theme's `material_level`, which is 0 in every stock theme, so nothing on screen changes until a stylesheet raises it: the `neumorphic`, `molded`, `glossy` and `milled` sheets in `widgets/themes/` do, and the desktop's style picker shows them.

Everything is computed in the pass that is already running, from the signed distance the shader already holds, through `mod.sdf.Material` in `draw/src/shader/surface.rs`. Nothing samples a texture or reads the clock, so a window at rest stays at rest. The same library lights `ReliefView`, whose relief buffer adds what a single pass cannot: the light one surface throws on its neighbours, and their occlusion of it. A control here does its own face and its own shadow only.

## Tiers

- **0** flat. Byte-identical to today.
- **1** relief: the shoulder, the cast shadow with its contact ring and light-side lip, a well's inner shadow.
- **2** adds the rim band, the gloss sweep and the specular highlight.

## The tokens

All under `material_` in the theme, each documented and ranged for the tweaker. The key light (`material_light_x/y/z`, `_intensity`) is shared by every control on screen, which is what makes a page cohere. The relief (`bevel_width`, `bevel_curve`, `raise`, `sink`), the finish (`specular`, `roughness`, `ao`, `rim`, `gloss`), the face (`face_gradient`, `hairline`, `ao_reach`), the shadow (`shadow`, `shadow_blur`, `shadow_falloff`, `contact_ao`, `ground_lip`), the well (`inner_shadow`, `inner_radius`), the glow (`glow`, `ink_glow`, `ink_lift`) and the press (`press_depth`, `press_invert`), with three inks: `color_material_light`, `color_material_shadow`, `color_material_glow`.

A control's shader takes them packed, four to a uniform: `material_light` is the light's x, y, z and intensity; `material_relief` the bevel width, bevel curve, raise and specular; `material_finish` the occlusion, rim, gloss and roughness; `material_tune` the face gradient, hairline, occlusion reach and sink; `material_shadow` the cast shadow, its blur, falloff and contact occlusion; `material_inner` the inner shadow, its blur, the ground lip and the glow.

## Where the shadow goes

Makepad clips by default, and a button is not a view, so it cannot grow past its rect. A material control keeps its quad and insets its face by `material_margin` instead; the shadow falls into that margin, and the stylesheet pays for it again in the control's padding. The rounded view uses its own `border_inset` for the same room. A shadow's blur is held to what the margin can show, so it fades out before the quad edge instead of ending on a straight line.

## What a press does

One elevation per control: raised at rest, lifted a quarter more under the pointer, moved by `material_press_depth` while held, moulded flat when disabled. The sign and size of the press pick the idiom -- past `-material_raise` the face inverts, short of it the cap deepens, near zero the glow carries the state -- and `material_press_invert` says whether a held face dishes as well as descends. The button's state layer (`layer_color`) is skipped under a material: a face that both darkens and re-lights reads as muddy.

Lit ink is one `mix` in the label's existing `get_color`, toward `color_material_glow` and past full brightness by `material_ink_lift`; the halo comes from the face. Nothing samples a glyph twice.

## What this page shows

Every instance here sets the whole material itself, because the catalogue's own theme leaves it off: the material bench's neumorphic preset, at tier 2 rather than the bench's 1 so the rim, gloss and specular controls have something to show.

The Controls tab has the bench's controls in the bench's groups, each a heading that folds its controls away or back. **Rest cap** is the tier, margin and press of the first button alone. **Material** picks one of the bench's eleven presets, which sets every control below it. **Light**, **Relief**, **Surface**, **Finish**, **Shadow** and **Colours** write their uniform on every material control on the page at once, a packed vector whole; the press controls move the inverting row only, and the illuminating row keeps its own glow, so the three press idioms stay apart.

The bench's metallic, clearcoat, environment, reflection, exposure and highlight roll-off, its groove and knob geometry, and its ground, body, label, cap and pointer inks have no shader or token behind them here, and are not offered.",
    subject: "subject",
    feature: None,
    controls: &[
        section("Rest cap"),
        Control { label: "Tier", target: "subject", kind: ControlKind::Number { prop: "draw_bg.material", min: 0., max: 2., step: 1., default: 2. } },
        Control { label: "Margin", target: "subject", kind: ControlKind::Number { prop: "draw_bg.material_margin", min: 0., max: 32., step: 1., default: 10. } },
        Control { label: "Press", target: "subject", kind: ControlKind::Number { prop: "draw_bg.material_press", min: -32., max: 8., step: 0.5, default: -9. } },
        Control { label: "Invert", target: "subject", kind: ControlKind::Number { prop: "draw_bg.material_press_invert", min: 0., max: 1., step: 0.05, default: 1. } },
        section("Material"),
        Control { label: "Preset", target: "", kind: ControlKind::Preset { options: PRESET_NAMES, default: 0, values: material_preset } },
        section("Light"),
        number(LIGHT_X, EVERY, "draw_bg.material_light[0]", -1., 1., 0.01, -0.35),
        number(LIGHT_Y, EVERY, "draw_bg.material_light[1]", -1., 1., 0.01, -0.55),
        number(LIGHT_Z, EVERY, "draw_bg.material_light[2]", 0., 1., 0.01, 0.66),
        number(INTENSITY, EVERY, "draw_bg.material_light[3]", 0., 2., 0.05, 0.70),
        section("Relief"),
        number(TIER, EVERY, "draw_bg.material", 0., 2., 1., 2.),
        number(FACE_GRADIENT, EVERY, "draw_bg.material_tune[0]", 0., 1., 0.01, 0.45),
        number(HAIRLINE, EVERY, "draw_bg.material_tune[1]", 0., 1., 0.01, 0.),
        number(BEVEL_WIDTH, EVERY, "draw_bg.material_relief[0]", 0., 24., 0.5, 4.),
        number(PROFILE_CURVE, EVERY, "draw_bg.material_relief[1]", 0., 1., 0.05, 0.70),
        number(RAISE, EVERY, "draw_bg.material_relief[2]", 0., 24., 0.5, 4.),
        number(SINK, EVERY, "draw_bg.material_tune[3]", 0., 24., 0.5, 4.),
        number(PRESS_DEPTH, PRESSED, "draw_bg.material_press", -32., 8., 0.5, -9.),
        number(PRESS_DISHES, PRESSED, "draw_bg.material_press_invert", 0., 1., 0.01, 1.),
        section("Surface"),
        number(ROUGHNESS, EVERY, "draw_bg.material_finish[3]", 0., 1., 0.01, 0.85),
        number(SPECULAR, EVERY, "draw_bg.material_relief[3]", 0., 1., 0.01, 0.),
        section("Finish"),
        number(OCCLUSION, EVERY, "draw_bg.material_finish[0]", 0., 1., 0.01, 0.50),
        number(OCCLUSION_REACH, EVERY, "draw_bg.material_tune[2]", 0.25, 3., 0.05, 1.20),
        number(RIM, EVERY, "draw_bg.material_finish[1]", 0., 1., 0.01, 0.40),
        number(GLOSS, EVERY, "draw_bg.material_finish[2]", 0., 1., 0.01, 0.),
        number(GLOW, INNER, "draw_bg.material_inner[3]", 0., 1., 0.01, 0.),
        number(INK_GLOW, LIT_LABEL, "draw_text.material_ink_glow", 0., 1., 0.01, 0.),
        section("Shadow"),
        number(CAST_SHADOW, EVERY, "draw_bg.material_shadow[0]", 0., 1., 0.01, 0.85),
        number(SHADOW_BLUR, EVERY, "draw_bg.material_shadow[1]", 0.5, 40., 0.5, 12.),
        number(FALLOFF, EVERY, "draw_bg.material_shadow[2]", 0., 1., 0.05, 1.),
        number(CONTACT, EVERY, "draw_bg.material_shadow[3]", 0., 1., 0.01, 0.50),
        number(INNER_SHADOW, INNER, "draw_bg.material_inner[0]", 0., 1., 0.01, 0.55),
        number(INNER_BLUR, INNER, "draw_bg.material_inner[1]", 0.5, 32., 0.5, 10.),
        number(GROUND_LIP, INNER, "draw_bg.material_inner[2]", 0., 1., 0.01, 0.70),
        section("Colours"),
        Control { label: LIGHT_INK, target: EVERY, kind: ControlKind::Color { prop: "draw_bg.material_light_ink", default: 0xFFFFFFFF } },
        Control { label: SHADOW_INK, target: EVERY, kind: ControlKind::Color { prop: "draw_bg.material_shadow_ink", default: 0x9299B3FF } },
        Control { label: GLOW_INK_LABEL, target: GLOW_INK, kind: ControlKind::Color { prop: "draw_bg.material_glow_ink", default: 0x7C4DFFFF } },
    ],
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is built from the DSL, which the Rust compiler never reads,
    /// and a shader that fails to compile is not an error anywhere -- the
    /// draw is skipped and the widget paints nothing. Building the page and
    /// asking for every widget the controls panel addresses is what turns a
    /// mistake in either into a failed build.
    #[test]
    fn the_page_builds_and_its_subject_can_be_reached() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile");
        // A control may write several widgets, named apart by spaces; a
        // section or a preset names none.
        let targets = story.controls.iter().flat_map(|c| c.target.split_whitespace());
        for target in std::iter::once(story.subject).chain(targets) {
            assert!(!page.widget(&cx, &[LiveId::from_str(target)]).is_empty(), "no widget at {}", target);
        }
    }
}
