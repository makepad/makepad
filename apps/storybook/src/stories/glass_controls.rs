//! The glass controls story: four controls that show you what is behind
//! them, and therefore need something to be behind them.
use crate::makepad_widgets::*;
use crate::registry::Story;

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
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
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

The two rows on this page are the same four declarations over a colourful ground and over one colour. The lens refracts the scene underneath, so over one colour it has nothing to bend and each control collapses to a faint outline; Glass > Overview says when that makes the family the wrong choice.

They must also be drawn in the same pass as what they refract. The glass example in this repository puts its content in the background pass rather than in a `glass.Layer` for exactly that reason — a layer would hide the base from the lens, and each toggle needs to refract its own track and knob.

`GlassButtonProminent` is the filled variant of `GlassButton`; the rest take the shapes you would expect — `labels` on the segmented row, a value on the slider.",
    subject: "subject",
    feature: None,
    controls: &[],
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
        assert!(
            !page.widget(&cx, &[LiveId::from_str(story.subject)]).is_empty(),
            "no widget at {}",
            story.subject
        );
    }
}
