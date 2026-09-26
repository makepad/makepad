//! The glass family's first page: the panel, the material every other glass
//! page is made of, over something worth bending and over nothing.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.GlassPanelOverview = StoryPage{
        StoryNote{text: "A sheet of glass: it samples the window behind itself, blurs it, and bends that through its own rounded edge. Everything it looks like comes from what is behind it, which is why it is shown here over a ground and not over the page."}

        StoryHeading{text: "Default"}
        StoryNote{text: "Drive the controls panel and watch this one. Every number the material takes is a shader input on draw_bg, so the panel opposite edits it the same way a page would."}
        GlassStage{
            height: 210.
            body +: {
                subject := GlassPanel{
                    width: 260
                    height: 140
                    padding: theme.mspace_3
                    flow: Down
                    spacing: theme.space_1
                    Label{text: "GlassPanel"}
                    Label{text: "Default material"}
                }
            }
        }

        StoryHeading{text: "Custom tint + edge"}
        StoryNote{text: "The same widget with seven values merged into draw_bg. Note the corner: the SDF box draws a visual radius of twice the number, so corner_radius 18 is a 36pt corner."}
        GlassStage{
            height: 210.
            body +: {
                GlassPanel{
                    width: 260
                    height: 140
                    padding: theme.mspace_3
                    flow: Down
                    spacing: theme.space_1
                    draw_bg +: {
                        tint_color: #6af
                        tint_alpha: 0.23
                        border_color: #9cf
                        border_alpha: 0.6
                        border_width: 1.5
                        corner_radius: 18.0
                        specular_strength: 0.5
                        noise_strength: 0.025
                    }
                    Label{text: "Cool tint"}
                    Label{text: "Rounded edge + stronger specular"}
                }
            }
        }

        StoryHeading{text: "The same panel over one colour"}
        StoryNote{text: "Identical declaration, nothing behind it to bend. This is what the family looks like on a plain page, and it is worth seeing before choosing it for one: the refraction has nothing to work with and the panel is left as a border, a specular rim and a shadow."}
        flat := FlatStage{
            height: 210.
            body +: {
                GlassPanel{
                    width: 260
                    height: 140
                    padding: theme.mspace_3
                    flow: Down
                    spacing: theme.space_1
                    Label{text: "GlassPanel"}
                    Label{text: "nothing behind it"}
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "containers/glass/overview",
        category: "Containers",
        component: "Glass",
        also: &["GlassPanel"],
        name: "Overview",
        dsl: "GlassPanelOverview",
        added: "2026-03-12",
        tags: &["ported"],
        doc: "# Glass

A family of surfaces that sample the window behind themselves through a chain of mip textures, blur it, and bend that through their own rounded edge. `GlassPanel`, the sheet on this page, is the plain one; every other member is the same material cut to a job.

**It draws what is behind it, so something has to be behind it.** Over a flat page the refraction has nothing to work with and the panel is left as a border, a specular rim and a shadow — which is why every demo here stands on a coloured ground, and why the last one deliberately does not. The ground also has to reach PAST the glass: the rim samples along the outward normal by up to `lensing_strength` points, so a ground that stops at the edge hands the bend the page background instead.

## Which one to use

| You want | Use | Page |
|---|---|---|
| a still sheet with content on it | `GlassPanel` | this one |
| a backing to build a glass control of your own on | `LensSurface` and its presets | Surfaces |
| a sheet that opens over the page and closes again | a popup backed by `GaussRoundedView` | Sheets |
| a sheet a person moves and sizes | `FloatingSurface` | Floating surface |
| a button, toggle, slider or segmented row that is glass already | `GlassButton` and its siblings | Controls |

None of them, if the page behind has nothing worth bending: over one colour every member is left as a border, a rim and a shadow, which a plain card draws without any of the cost set out below.

## The knobs

`blur_level` is a level in that pyramid, 0 to 6, not a radius in pixels. Level 0 is the captured scene at full size and level N reads a texture 1/2^N of the window; values between two levels blend. The deep levels are carried back up to a 1/8-resolution floor before the glass reads them, so no sample ever comes from a texture coarser than eight device pixels per texel.

`lensing_effect` (0..1) is how much of the edge bend to apply, `lensing_strength` how far it bends, and `lensing_width` how wide the band at the edge is — capped in the shader at about a third of the shorter side, so a small surface does not become all edge. `diffraction_strength` splits red, green and blue to slightly different lookups, which is the colour fringe at the rim.

`corner_radius` is **half the visual radius**: the SDF box draws a corner twice the number you give it.

The rest are surface: `tint_color` with `tint_alpha`, `border_color`/`border_alpha`/`border_width`, `specular_strength` for the rim light, `noise_strength` for the de-banding grain, and `shadow_color`/`shadow_alpha`/`shadow_radius`/`shadow_offset` for what it casts.

`fallback_color` is the face it shows when there is no scene to sample — before the first capture, or with `MAKEPAD_NO_GAUSS=1` set. It is mixed from the theme's background and text colours, so it stays visible as a slab whichever theme is running.

## What it costs

A window with any glass on it renders its own body to a texture and builds a blur pyramid from it every frame: thirteen extra passes — one capture, six downsamples, six that carry the deep levels back up — plus a full-screen composite, and up to twenty-four texture fetches per glass pixel. Full-window supersampling is switched off for that window while it runs. A window with no glass in it never captures at all. Reach for this family where it earns that, not by default.

`MAKEPAD_NO_GAUSS=1` switches the capture off and shows every surface's fallback face, which is the way to check that face. `MAKEPAD_GAUSS_FAST=1` is a pass-count probe and **not** a like-for-like picture above `blur_level` 3: it stops the chain early, so the levels this family uses are either un-smoothed or never rendered at all.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Blur level",    target: "subject", kind: ControlKind::Number { prop: "draw_bg.blur_level",           min: 0.,  max: 6.,   step: 0.1,   default: 5.2 } },
            Control { label: "Refraction",    target: "subject", kind: ControlKind::Number { prop: "draw_bg.lensing_effect",       min: 0.,  max: 1.,   step: 0.02,  default: 0.94 } },
            Control { label: "Bend",          target: "subject", kind: ControlKind::Number { prop: "draw_bg.lensing_strength",     min: 0.,  max: 60.,  step: 1.,    default: 28. } },
            Control { label: "Edge band",     target: "subject", kind: ControlKind::Number { prop: "draw_bg.lensing_width",        min: 1.,  max: 48.,  step: 1.,    default: 20. } },
            Control { label: "Colour split",  target: "subject", kind: ControlKind::Number { prop: "draw_bg.diffraction_strength", min: 0.,  max: 12.,  step: 0.2,   default: 4.4 } },
            Control { label: "Corner radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.corner_radius",        min: 0.,  max: 32.,  step: 0.5,   default: 10. } },
            Control { label: "Tint",          target: "subject", kind: ControlKind::Color  { prop: "draw_bg.tint_color",           default: 0xF8FBFFFF } },
            Control { label: "Tint amount",   target: "subject", kind: ControlKind::Number { prop: "draw_bg.tint_alpha",           min: 0.,  max: 0.30, step: 0.002, default: 0.08 } },
            Control { label: "Border",        target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_alpha",         min: 0.,  max: 1.,   step: 0.02,  default: 0.72 } },
            Control { label: "Border width",  target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_width",         min: 0.,  max: 4.,   step: 0.25,  default: 1. } },
            Control { label: "Specular",      target: "subject", kind: ControlKind::Number { prop: "draw_bg.specular_strength",    min: 0.,  max: 1.,   step: 0.02,  default: 0.22 } },
            Control { label: "Grain",         target: "subject", kind: ControlKind::Number { prop: "draw_bg.noise_strength",       min: 0.,  max: 0.08, step: 0.002, default: 0.004 } },
            Control { label: "Shadow",        target: "subject", kind: ControlKind::Number { prop: "draw_bg.shadow_alpha",         min: 0.,  max: 1.,   step: 0.05,  default: 1.0 } },
            Control { label: "Shadow radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.shadow_radius",        min: 0.,  max: 32.,  step: 1.,    default: 13. } },
        ],
        on_actions: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Cx` with this crate's theme, the shared page templates and this
    /// file's own story registered, and nothing else: a failure here is
    /// this page's, not some other story's.
    fn shell() -> Cx {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            // Registering a template compiles nothing; making an instance
            // out of one does. Clearing here keeps any other module's
            // complaint out of these tests' answers.
            let _ = makepad_platform::shader_error::take();
        });
        cx
    }

    fn build(cx: &mut Cx, dsl: &str) -> WidgetRef {
        cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {dsl}");
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// The page is built from the DSL, which the Rust compiler never reads:
    /// a mistake in a `script_mod!` block shows up only in a running app's
    /// log, and a shader that fails to compile is not an error anywhere —
    /// the draw is skipped and the widget paints nothing. The page stands on
    /// a shared stage, so building it and asking for every id the controls
    /// panel addresses is what turns either mistake into a failed build.
    #[test]
    fn the_page_builds_and_its_subject_can_be_reached() {
        let mut cx = shell();
        for story in STORIES {
            let page = build(&mut cx, story.dsl);
            assert!(!page.is_empty(), "{} built no widget", story.key);
            assert_eq!(
                makepad_platform::shader_error::take(),
                None,
                "{}: a draw shader failed to compile",
                story.key
            );
            // Every id the controls panel writes to has to be findable, or
            // the row moves a slider that reaches nothing.
            for target in std::iter::once(story.subject)
                .chain(story.controls.iter().map(|c| c.target))
                .filter(|t| !t.is_empty())
            {
                assert!(
                    !page.widget(&cx, &[LiveId::from_str(target)]).is_empty(),
                    "{}: no widget at {}",
                    story.key,
                    target
                );
            }
        }
    }

    /// The flat stage REPLACES the coloured stage's two layers rather than
    /// adding two more.
    ///
    /// This is the trap the shared stage exists inside: a derived object
    /// copies its prototype's children and then appends its own, so an
    /// anonymous child in `FlatStage` would leave the coloured ground
    /// switched on under a fourth layer and paint an opaque fifth one over
    /// the demo — in an Overlay flow the last layer is on top. Nothing warns
    /// about it and it compiles perfectly, so the layers are counted here.
    #[test]
    fn the_flat_stage_overrides_its_layers_instead_of_adding_more() {
        let mut cx = shell();
        let page = build(&mut cx, "GlassPanelOverview");
        let stage = page.widget(&cx, &[live_id!(flat)]);
        assert!(!stage.is_empty(), "the flat stage is named on the page");
        let view = stage.borrow::<View>().expect("a stage is a View");
        let layers: Vec<LiveId> = view.children.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            layers,
            vec![live_id!(ground), live_id!(scrim), live_id!(body)],
            "three layers, in paint order, and the demo slot last"
        );
        let ground = view
            .children
            .iter()
            .find(|(id, _)| *id == live_id!(ground))
            .map(|(_, w)| w.clone())
            .unwrap();
        assert!(
            !ground.borrow::<View>().expect("the ground is a View").visible,
            "the flat stage switched the coloured ground off"
        );
    }

    /// A number control's default has to be a value its own slider can
    /// hold. A default outside the range is silently clamped on the first
    /// drag, so the row jumps the moment it is touched — and the panel
    /// fills each row exactly once, so nothing ever puts it back.
    ///
    /// Scoped to this file's own stories. The same invariant is worth
    /// pinning across the whole catalogue, but that test belongs next to
    /// the registry these records are declared against.
    #[test]
    fn number_controls_can_hold_their_own_defaults() {
        for story in STORIES {
            for control in story.controls {
                if let ControlKind::Number { prop, min, max, step, default } = &control.kind {
                    assert!(min < max, "{} {}: min {} is not below max {}", story.key, prop, min, max);
                    assert!(*step > 0., "{} {}: step {} does not move", story.key, prop, step);
                    assert!(
                        min <= default && default <= max,
                        "{} {}: default {} is outside {}..{}",
                        story.key, prop, default, min, max
                    );
                }
            }
        }
    }
}
