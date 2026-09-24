//! The surface material story: the moulded relief the base shaders carry,
//! switched on here whatever theme the catalogue is showing.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // The material switched on and given a shadow and a margin, whatever
    // theme the catalogue is showing. Every stock theme carries the
    // material's defaults at material_level 0, so a page has to raise the
    // tier and give the shadow a strength itself; under one of the material
    // sheets the same numbers already come from the theme and these only
    // repeat them. The margin is the room the shadow falls into, paid for
    // again in the padding, since a control keeps its quad and insets its
    // face rather than growing past its clip.
    let Cap = Button{
        padding: Inset{left: 22. right: 22. top: 16. bottom: 16.}
        draw_bg +: {
            material: 2.0
            material_margin: 10.0
            material_shadow: vec4(0.6, 10.0, 1.0, 0.4)
            material_inner: vec4(0.6, 8.0, 0.0, 0.0)
            border_size: 0.0
        }
    }
    // The three press idioms, one signed number each.
    let Invert = Cap{draw_bg +: {material_press: -9.0 material_press_invert: 1.0}}
    let Deepen = Cap{draw_bg +: {material_press: -2.0 material_press_invert: 0.0}}
    let Illuminate = Cap{
        draw_bg +: {material_press: -1.0 material_press_invert: 0.0 material_inner: vec4(0.6, 8.0, 0.0, 0.6)}
        draw_text +: {material_ink_glow: 0.85}
    }

    let Well = CheckBox{draw_bg +: {material: 2.0 material_inner: vec4(0.6, 8.0, 0.0, 0.0) border_size: 0.0}}
    let Pill = Toggle{draw_bg +: {material: 2.0 material_inner: vec4(0.6, 8.0, 0.0, 0.0) material_shadow: vec4(0.5, 6.0, 1.0, 0.4) border_size: 0.0}}
    let Groove = Slider{
        width: Fill
        draw_bg +: {material: 2.0 material_inner: vec4(0.6, 8.0, 0.0, 0.0) material_shadow: vec4(0.5, 8.0, 1.0, 0.4) border_size: 0.0}
    }
    let Knob = RotaryKnob{
        text: ""
        draw_bg +: {material: 2.0 material_margin: 8.0 material_shadow: vec4(0.6, 8.0, 1.0, 0.5) border_size: 0.0}
    }

    mod.stories.SurfaceMaterialOverview = StoryPage{
        StoryNote{text: "The base shaders carry a moulded material behind one uniform. At 0, every stock theme, they draw what they always drew; at 1 a face is lit from its own distance field (a shoulder, a cast shadow, a well's inner shadow); at 2 it also takes a rim, a gloss sweep and a specular. This page raises the tier on each control, whatever theme is showing, so the relief can be read against the flat page around it."}

        StoryHeading{text: "Raised and sunken"}
        StoryNote{text: "The same material lit from opposite sides. A rounded view stands off the page: its face is lit and its cast shadow, contact ring and light-side lip go UNDER the face, inside the margin border_inset reclaims. A panel is a shallow step in the housing; sunken, it takes the surround's shadow across its face."}
        StoryRow{
            spacing: theme.space_3
            RoundedView{
                width: 160. height: 90.
                draw_bg +: {
                    color: theme.color_outset
                    border_radius: 8.
                    border_inset: vec4(12. 12. 12. 12.)
                    material: 2.0
                    material_shadow: vec4(0.6, 10.0, 1.0, 0.4)
                }
            }
            PanelView{width: 160. height: 90. draw_bg +: {material: 2.0}}
            InsetPanelView{width: 160. height: 90. draw_bg +: {material: 2.0 material_inner: vec4(0.6, 8.0, 0.0, 0.0)}}
        }

        StoryHeading{text: "A button that really goes down"}
        StoryNote{text: "Rest, hover, pressed, focused, disabled. The pointer lifts a cap a quarter more; a press adds material_press, and its sign and size pick the idiom. Disabled moulds the cap flat into the page, which also takes its shadow away. The focus ring stays what it was: focus is a ring, never a change of relief."}
        StoryNote{text: "INVERT: past minus the raise the face crosses zero, the lit and shaded shoulders swap and the cap reads as pushed into the page. material_press_invert dishes the face as well. The neumorphic sheet."}
        StoryRow{
            subject := Invert{text: "Rest"}
            Invert{text: "Hover" animator +: {hover: {default: @on}}}
            Invert{text: "Pressed" animator +: {hover: {default: @down}}}
            Invert{text: "Focused" animator +: {focus: {default: @on}}}
            Invert{text: "Disabled" animator +: {disabled: {default: @on}}}
        }
        StoryNote{text: "DEEPEN: the cap drops but stays above the page, its face still convex, so the shadow closes up and the specular dims. Moulded plastic flexes; it does not turn inside out. The molded sheet."}
        StoryRow{
            Deepen{text: "Rest"}
            Deepen{text: "Hover" animator +: {hover: {default: @on}}}
            Deepen{text: "Pressed" animator +: {hover: {default: @down}}}
            Deepen{text: "Focused" animator +: {focus: {default: @on}}}
            Deepen{text: "Disabled" animator +: {disabled: {default: @on}}}
        }
        StoryNote{text: "ILLUMINATE: the cap barely moves and the glow carries the state -- the face lifts toward the glow ink, a halo spills onto the page, and the label lights with it, pushed past full brightness so it clips bright rather than tints. The glossy and milled sheets."}
        StoryRow{
            Illuminate{text: "Rest"}
            Illuminate{text: "Hover" animator +: {hover: {default: @on}}}
            Illuminate{text: "Pressed" animator +: {hover: {default: @down}}}
            Illuminate{text: "Focused" animator +: {focus: {default: @on}}}
            Illuminate{text: "Disabled" animator +: {disabled: {default: @on}}}
        }

        StoryHeading{text: "Wells and caps"}
        StoryNote{text: "A check box is a well cut into the housing. A toggle is a sunken track with one solid knob travelling in it, the same substance as the housing, throwing its shadow on the track and taking the active ink as it turns on. A slider's track is a groove and its handle a cap standing in it."}
        StoryRow{
            spacing: theme.space_3
            Well{text: "Off"}
            Well{text: "On" active: true}
            Pill{text: "Off"}
            Pill{text: "On" active: true}
        }
        StoryRow{
            Groove{text: "Level"}
        }
        StoryNote{text: "The knob is a domed cap: the shoulder rolls into a shallow dome so the whole face catches the light, and it keeps back from its box by the margin its shadow falls into, held to a third of the radius so a 24-square knob keeps a face. At 24, 32, 44 and 64."}
        StoryRow{
            spacing: theme.space_3
            align: Align{x: 0. y: 1.}
            Knob{width: 24. height: 24.}
            Knob{width: 32. height: 32.}
            Knob{width: 44. height: 44.}
            Knob{width: 64. height: 64.}
        }
    }
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

## Where the shadow goes

Makepad clips by default, and a button is not a view, so it cannot grow past its rect. A material control keeps its quad and insets its face by `material_margin` instead; the shadow falls into that margin, and the stylesheet pays for it again in the control's padding. The rounded view uses its own `border_inset` for the same room. A shadow's blur is held to what the margin can show, so it fades out before the quad edge instead of ending on a straight line.

## What a press does

One elevation per control: raised at rest, lifted a quarter more under the pointer, moved by `material_press_depth` while held, moulded flat when disabled. The sign and size of the press pick the idiom -- past `-material_raise` the face inverts, short of it the cap deepens, near zero the glow carries the state -- and `material_press_invert` says whether a held face dishes as well as descends. The button's state layer (`layer_color`) is skipped under a material: a face that both darkens and re-lights reads as muddy.

Lit ink is one `mix` in the label's existing `get_color`, toward `color_material_glow` and past full brightness by `material_ink_lift`; the halo comes from the face. Nothing samples a glyph twice.

## What this page shows

Every instance here sets `material: 2.0` and a shadow strength of its own, because the catalogue's own theme leaves both at 0. Pick one of the material sheets from the style menu to see the same controls with the theme's numbers instead.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Tier", target: "subject", kind: ControlKind::Number { prop: "draw_bg.material", min: 0., max: 2., step: 1., default: 2. } },
        Control { label: "Margin", target: "subject", kind: ControlKind::Number { prop: "draw_bg.material_margin", min: 0., max: 32., step: 1., default: 10. } },
        Control { label: "Press", target: "subject", kind: ControlKind::Number { prop: "draw_bg.material_press", min: -32., max: 8., step: 0.5, default: -9. } },
        Control { label: "Invert", target: "subject", kind: ControlKind::Number { prop: "draw_bg.material_press_invert", min: 0., max: 1., step: 0.05, default: 1. } },
    ],
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is built from the DSL, which the Rust compiler never reads,
    /// and a shader that fails to compile is not an error anywhere -- the
    /// draw is skipped and the widget paints nothing. Building the page and
    /// asking for the widget the controls panel addresses is what turns a
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
        for target in std::iter::once(story.subject).chain(story.controls.iter().map(|c| c.target)) {
            assert!(!page.widget(&cx, &[LiveId::from_str(target)]).is_empty(), "no widget at {}", target);
        }
    }
}
