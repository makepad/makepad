//! The glass controls story: four controls that show you what is behind
//! them, and therefore need something to be behind them.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.GlassControlsOverview = StoryPage{
        StoryNote{text: "Four controls that bend what is behind them. They are the panel's family: the same lensing, applied to a toggle, a button, a slider and a segmented row."}

        StoryHeading{text: "Over something worth bending"}
        StoryNote{text: "The colours under these are drawn by the page, not by the controls. Put one on a flat ground and the lensing has nothing to work with, which is the commonest way this family disappoints."}
        // 260 and not the old 230: the shared stage spends 60 points of
        // height on the clearance the rim needs, where the old inset spent
        // 18, and the four rows plus their spacing come to 184.
        GlassStage{
            height: 260.
            body +: {
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    radio_one := mod.widgets.glass.GlassRadio{}
                    mod.widgets.glass.OptionLabel{text: "Air"}
                    radio_two := mod.widgets.glass.GlassRadio{}
                    mod.widgets.glass.OptionLabel{text: "Water"}
                }
                // Unclipped, so the buttons' shadows show in full: a row that
                // clips at its Fit height leaves them no room under the
                // buttons, and the shadow thins out to nothing there.
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    clip_x: false clip_y: false
                    mod.widgets.glass.GlassButtonProminent{text: "Continue"}
                    subject := mod.widgets.glass.GlassButton{text: "Cancel"}
                }
                mod.widgets.glass.GlassSlider{width: Fill}
                mod.widgets.glass.GlassSegmented{
                    width: Fill
                    labels: ["Day" "Week" "Month"]
                }
            }
        }

        StoryHeading{text: "A lens you press"}
        StoryNote{text: "The button's glass is a water lens. Hold one and the lens lies flat under the finger behind a ring that crosses it; let go and it springs back behind a softer ring, a little past rest, before it settles. The click is sent when you let go, and the rebound plays after it. This one is 300 by 92 and carries the lens button's own numbers, written on draw_glass under the names the glass surfaces use, to compare with the pressable lens on Glass > Sheets."}
        GlassStage{
            height: 240.
            body +: {
                align: Align{x: 0.5 y: 0.5}
                focus_lens := mod.widgets.glass.GlassButton{
                    width: 300
                    height: 92
                    text: "Focus"
                    draw_text +: {text_style: theme.font_regular{font_size: 22}}
                    draw_glass +: {
                        blur_level: 0.25
                        lensing_effect: 1.0
                        lensing_strength: 38.0
                        lensing_width: 13.0
                        corner_radius: 23.0
                        tint_color: #b8b8b8
                        tint_alpha: 0.025
                        border_alpha: 0.82
                        specular_strength: 0.24
                        noise_strength: 0.004
                        shadow_color: #0009
                        shadow_radius: 34.0
                        shadow_offset: vec2(0.0, 14.0)
                        diffraction_strength: 5.2
                    }
                }
            }
        }

        StoryHeading{text: "The same four on a flat ground"}
        StoryNote{text: "Identical declarations, over one colour. This is what the family looks like when there is nothing behind it to refract, and it is worth seeing next to the row above before choosing it for a page that has a plain background."}
        FlatStage{
            height: 260.
            body +: {
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.GlassRadio{}
                    mod.widgets.glass.OptionLabel{text: "Air"}
                }
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    clip_x: false clip_y: false
                    mod.widgets.glass.GlassButtonProminent{text: "Continue"}
                    mod.widgets.glass.GlassButton{text: "Cancel"}
                }
                mod.widgets.glass.GlassSlider{width: Fill}
                mod.widgets.glass.GlassSegmented{
                    width: Fill
                    labels: ["Day" "Week" "Month"]
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/glass/controls",
    category: "Containers",
    component: "Glass",
    also: &["GlassButton", "GlassButtonProminent", "GlassRadio", "GlassSegmented", "GlassSlider", "OptionLabel"],
    name: "Controls",
    dsl: "GlassControlsOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Glass controls

A toggle, a button, a slider and a segmented row, all built on the same lensing the glass panel uses. They live under `mod.widgets.glass`, not at the top of the widget module, so they are written `glass.GlassButton` and so on.

The first and last bands on this page are the same four declarations over a colourful ground and over one colour. The lens refracts the scene underneath, so over one colour it has nothing to bend and each control collapses to a faint outline; Glass > Overview says when that makes the family the wrong choice.

They must also be drawn in the same pass as what they refract. The glass example in this repository puts its content in the background pass rather than in a `glass.Layer` for exactly that reason — a layer would hide the base from the lens, and each toggle needs to refract its own track and knob.

`GlassButtonProminent` is the filled variant of `GlassButton`; the rest take the shapes you would expect — `labels` on the segmented row, a value on the slider.

## What a press does

`GlassButton` is drawn on the water lens, `RippleLensRoundedView`: a sheen toward the rim and the top, a bright seal, a colour split at the rim and a soft shadow under it. Its glass is that preset's own draw rather than a copy, so every knob of the lens is written on `draw_glass` under the name it has on the surface, and the numbers of one paste onto the other.

- **Press.** The lens lies flat under the finger over `press_secs` (0.78 s), behind a ring that crosses it in 0.88 s and fades over `ripple_secs` (1.05 s). Held on after that, the lens stays flat and still.
- **Release.** The lens springs back from as flat as it got, behind a second ring `release_ripple` (0.62) as strong, lifting a little past rest just behind the ring before it settles about a second later.
- **Click.** `clicked` and `on_click` fire on the release, at the same moment they always did; the rebound plays after them, not before.
- **Label.** The label and the icon dim to `hover_ink` (0.65 of their alpha) under the pointer and to `down_ink` (0.25) while held, as the lens button's label does. The glass itself does not light up.
- **Disabled.** A button put out of use with `set_disabled` takes no press and sends no click, and shows its label and icon at `disabled_ink` (0.4).
- **Cost.** The clock runs only while the lens is flattening or rebounding, so a button at rest, or one held down after its ring has gone, asks for no frames.
- **`reduced_motion: true`.** The lens goes flat on the press and back on the release at once, with no ring and no clock. The controls panel has a switch for it on the Cancel button and on the 300 by 92 one.

A 44 point button carries the lens button's numbers scaled to its height, 44/92 of everything measured in points: an 18 point bend over a 6.2 point band, a 2.5 point colour split, a shadow 16 points wide and 6.7 points down. Nothing shrinks under the finger; the flatten and the ring are the press. The label is 13 point bold and white. The 300 by 92 button on this page carries the unscaled numbers.

Like every glass surface, the button paints past its own rect: the shadow below it and the rim's bend around it. A container clips what is drawn in it, so the shadow keeps to the room its container leaves under the button. In a row only as tall as its buttons it thins out to nothing rather than being cut into a slab with hard edges, and wherever a clip still crosses it, it fades out before the edge. The rows on this page switch `clip_x` and `clip_y` off so the shadow shows in full.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Reduced motion", target: "subject",    kind: ControlKind::Bool { prop: "reduced_motion", default: false } },
        Control { label: "Lens reduced motion", target: "focus_lens", kind: ControlKind::Bool { prop: "reduced_motion", default: false } },
    ],
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is built from the DSL, which the Rust compiler never reads,
    /// and a shader that fails to compile is not an error anywhere — the
    /// draw is skipped and the widget paints nothing. Both bands moved onto
    /// a shared stage that did not exist before, so building the page and
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
        assert_eq!(
            makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        for target in std::iter::once(story.subject).chain(story.controls.iter().map(|c| c.target)) {
            assert!(
                !page.widget(&cx, &[LiveId::from_str(target)]).is_empty(),
                "no widget at {}",
                target
            );
        }
    }
}
