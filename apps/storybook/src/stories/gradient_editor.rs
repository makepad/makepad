//! The gradient editor story: a colour ramp edited on the ramp itself, the
//! presets, the three blend spaces, and a ramp read back under a slider.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Readout = Label{
        width: Fit
        height: Fit
        draw_text +: {color: theme.color_on_surface_variant}
    }

    let Caption = Label{
        width: Fit
        height: Fit
        draw_text +: {color: theme.color_on_surface_variant}
    }

    let Stack = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_1
    }

    mod.stories.GradientEditorOverview = StoryPage{
        StoryNote{text: "A colour ramp, edited on the ramp. Colour marks hang under the bar and alpha marks stand over it. Press the bar or either row of marks to add a mark there — it takes the colour the ramp already has, so nothing changes until you move or edit it — and keep dragging to place it. Drag a mark along the bar to move it, or well away from the bar to remove it. Double press a colour mark to open the colour picker on it."}

        StoryHeading{text: "A ramp"}
        StoryRow{
            width: Fill
            subject := GradientEditor{
                width: Fill
                stops: [
                    ColorStop{t: 0.0 color: #x1d2b53}
                    ColorStop{t: 0.5 color: #xe0457b}
                    ColorStop{t: 1.0 color: #xffd166}
                ]
                alpha_stops: [
                    AlphaStop{t: 0.0 alpha: 1.0}
                    AlphaStop{t: 1.0 alpha: 0.35}
                ]
            }
        }
        StoryRow{
            spacing: theme.space_2
            Caption{text: "Load a preset"}
            load_fire := Button{text: "Fire"}
            load_spectrum := Button{text: "Spectrum"}
            load_fade := Button{text: "Fade"}
            load_ember := Button{text: "Ember"}
        }
        StoryRow{
            edit_read := Readout{text: "no edit yet"}
        }

        StoryHeading{text: "Reading it back"}
        StoryNote{text: "`sample(t)` is the colour times the intensity, with the alpha from the alpha stops. The swatch shows what a display can; the numbers show the value, which may run past 1 where a mark's intensity does."}
        StoryRow{
            width: Fill
            spacing: theme.space_3
            probe := Slider{width: 260. text: "Sample at" min: 0. max: 1. default: 0.5}
            probe_swatch := ColorSwatch{width: 40 height: 22 color: #x808080FF}
            probe_read := Readout{text: "move the slider to sample the ramp"}
        }

        StoryHeading{text: "Presets"}
        StoryNote{text: "A `GradientEditor` whose `stops` are left empty takes them from its `preset`, and the same for `alpha_stops`. Ember runs its hot end at four times white, which is what the intensity field is for; the bar can only show it clipped."}
        Stack{
            Caption{text: "GradientPreset.Fire"}
            GradientEditor{width: Fill preset: GradientPreset.Fire show_fields: false}
            Caption{text: "GradientPreset.Ocean"}
            GradientEditor{width: Fill preset: GradientPreset.Ocean show_fields: false}
            Caption{text: "GradientPreset.Ember"}
            GradientEditor{width: Fill preset: GradientPreset.Ember show_fields: false}
        }

        StoryHeading{text: "Where the blend happens"}
        StoryNote{text: "The same two stops, red and green, blended three ways. Srgb mixes the stored numbers, which is what most tools do and sags to a muddy dark middle; Linear mixes light; Oklab mixes in a perceptual space, which also keeps the lightness even from end to end."}
        Stack{
            Caption{text: "GradientSpace.Srgb"}
            GradientEditor{
                width: Fill
                show_fields: false
                space: GradientSpace.Srgb
                stops: [ColorStop{t: 0.0 color: #xff0000} ColorStop{t: 1.0 color: #x00ff00}]
            }
            Caption{text: "GradientSpace.Linear"}
            GradientEditor{
                width: Fill
                show_fields: false
                space: GradientSpace.Linear
                stops: [ColorStop{t: 0.0 color: #xff0000} ColorStop{t: 1.0 color: #x00ff00}]
            }
            Caption{text: "GradientSpace.Oklab"}
            GradientEditor{
                width: Fill
                show_fields: false
                space: GradientSpace.Oklab
                stops: [ColorStop{t: 0.0 color: #xff0000} ColorStop{t: 1.0 color: #x00ff00}]
            }
        }
    }
}

/// What the ramp holds at the slider's position, as the page says it.
fn show_probe(cx: &mut Cx, root: &WidgetRef) {
    let t = root.slider(cx, ids!(probe)).value().unwrap_or(0.5);
    let c = root.gradient_editor(cx, ids!(subject)).sample(t);
    root.color_swatch(cx, ids!(probe_swatch))
        .set_color(cx, vec4(c.x.min(1.0), c.y.min(1.0), c.z.min(1.0), c.w));
    root.label(cx, ids!(probe_read)).set_text(
        cx,
        &format!(
            "at {t:.2}: r {:.2}  g {:.2}  b {:.2}  alpha {:.2}",
            c.x, c.y, c.z, c.w
        ),
    );
}

fn gradient_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let editor = root.gradient_editor(cx, ids!(subject));
    for (id, preset) in [
        (ids!(load_fire), GradientPreset::Fire),
        (ids!(load_spectrum), GradientPreset::Spectrum),
        (ids!(load_fade), GradientPreset::Fade),
        (ids!(load_ember), GradientPreset::Ember),
    ] {
        if root.button(cx, id).clicked(actions) {
            editor.set_preset(cx, preset);
            root.label(cx, ids!(edit_read))
                .set_text(cx, &format!("loaded {}", preset.name()));
            show_probe(cx, root);
        }
    }
    if let Some(g) = editor.ended(actions) {
        root.label(cx, ids!(edit_read)).set_text(
            cx,
            &format!(
                "recorded {} colour and {} alpha stops",
                g.stops.len(),
                g.alpha_stops.len()
            ),
        );
    }
    if editor.changed(actions).is_some() || root.slider(cx, ids!(probe)).slided(actions).is_some() {
        show_probe(cx, root);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/gradient-editor/overview",
    category: "Inputs",
    component: "GradientEditor",
    also: &["ColorStop", "AlphaStop", "GradientSpace", "GradientPreset"],
    name: "Overview",
    dsl: "GradientEditorOverview",
    added: "2026-09-25",
    tags: &["new", "controls", "gradient", "ramp", "colour", "hdr"],
    doc: "# GradientEditor\n\nA colour ramp, edited on the ramp itself: a bar showing the gradient, its colour marks under it and its alpha marks over it. The two kinds are kept apart on purpose — a colour and a transparency are almost never chosen at the same places along a ramp, and one mark carrying both forces a second mark wherever only one of them changes.\n\n## The gestures\n\n| Gesture | Result |\n|---|---|\n| press bare bar or a mark row | adds a mark there carrying what the ramp already has, and the drag carries on with it |\n| drag a mark | moves it; it may pass the others |\n| drag a mark well away from the bar | removes it when you let go; bring it back to keep it |\n| double press a colour mark | opens the colour picker on it |\n\nThe row over the bar and the checkered top half of the bar add alpha marks; the row under it and the solid bottom half add colour marks. The last mark of either kind cannot be removed.\n\n## Keyboard\n\nLeft and Right nudge the selected mark by `nudge` (ten times that with Shift), Delete or Backspace removes it, and Enter opens the picker on a colour mark.\n\n## The fields\n\nUnder the bar sits a row for the selected mark: the library's own `ColorPickerButton`, the mark's location in percent, and its intensity (a colour mark) or its alpha (an alpha mark). `show_fields: false` turns the row off; `with_intensity: false` hides the intensity field.\n\n## Intensity and blending\n\nEvery colour mark carries an HDR intensity of at least 1, up to `max_intensity`. `sample(t)` returns the interpolated colour times the interpolated intensity, with the alpha from the alpha stops; the bar shows the product clipped to what a display can show.\n\n`space` chooses how colours are blended: `GradientSpace.Srgb` mixes the stored values, `Linear` mixes light, and `Oklab` mixes in a perceptual space. The bar is drawn with the same blend `sample` uses.\n\n## Declaring one\n\n```\nGradientEditor{\n    stops: [\n        ColorStop{t: 0.0 color: #x1d2b53}\n        ColorStop{t: 1.0 color: #xffd166 intensity: 2.0}\n    ]\n    alpha_stops: [AlphaStop{t: 0.0 alpha: 1.0} AlphaStop{t: 1.0 alpha: 0.35}]\n    space: GradientSpace.Oklab\n}\n```\n\nLeave `stops` or `alpha_stops` out and they come from `preset`.\n\n## Reading it\n\n`changed` reports the ramp on every step of a drag; `ended` reports the ramp a gesture or an edit settled on — record that one. `selected` reports the mark the fields are editing.",
    subject: "subject",
    feature: None,
    controls: &[
        Control {
            label: "Blend",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "space",
                options: &["GradientSpace.Srgb", "GradientSpace.Linear", "GradientSpace.Oklab"],
                default: 0,
            },
        },
        Control { label: "Bar height", target: "subject", kind: ControlKind::Number { prop: "bar_height", min: 8., max: 64., step: 1., default: 26. } },
        Control { label: "Alpha band", target: "subject", kind: ControlKind::Number { prop: "alpha_band", min: 0., max: 1., step: 0.05, default: 0.5 } },
        Control { label: "Most intensity", target: "subject", kind: ControlKind::Number { prop: "max_intensity", min: 1., max: 64., step: 1., default: 16. } },
        Control { label: "Intensity field", target: "subject", kind: ControlKind::Bool { prop: "with_intensity", default: true } },
        Control { label: "Fields", target: "subject", kind: ControlKind::Bool { prop: "show_fields", default: true } },
    ],
    on_actions: Some(gradient_actions),
}];
