//! The drop controls story: three chips that keep their dragging in a
//! popover, because a drag in a toolbar is a fight nobody wins.
use crate::makepad_widgets::drop_slider::DropSliderWidgetRefExt;
use crate::makepad_widgets::drop_toggles::DropTogglesWidgetRefExt;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Bar = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{y: 0.5}
        padding: theme.mspace_2
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_low}
    }

    mod.stories.DropControlsOverview = StoryPage{
        StoryNote{text: "Three controls shaped for a crowded toolbar: each is a small chip that opens a panel under itself, and the panel is where the dragging happens."}

        StoryHeading{text: "A chip in the bar, the control underneath"}
        StoryNote{text: "Click any of these. The chip itself only ever takes a click; the slider you drag and the switches you flick live in a popover below it, and close when you press elsewhere."}
        StoryRow{
            Bar{
                Label{text: "Master" draw_text +: {color: theme.color_text_meta}}
                level := DropSlider{
                    min: 0.0
                    max: 1.2
                    default: 0.9
                    display_scale: 100.0
                    suffix: "%"
                }
                Label{text: "Filter" draw_text +: {color: theme.color_text_meta}}
                filters := DropToggles{
                    height: 22
                    text: "FILTER"
                    labels: ["Stems" "Karaoke" "Key" "Tempo"]
                }
                Label{text: "Source" draw_text +: {color: theme.color_text_meta}}
                source := DropDown2{
                    labels: ["Deck A" "Deck B" "Microphone" "Line in"]
                }
            }
        }
        StoryRow{
            reported := Label{text: "nothing touched yet"}
        }

        StoryHeading{text: "Why the drag is not in the bar"}
        StoryNote{text: "A control you drag sideways inside a window's own chrome competes with dragging the window. Whichever wins, the other feels broken, and the usual patch — a modifier key, a dead zone, a delay — makes the control worse to use. These move the drag one layer out instead: the bar keeps a plain click target, the popover gets a full-sized control with room to aim at."}

        StoryHeading{text: "They report what changed, not that something did"}
        StoryNote{text: "The slider reports its value, the toggles report which one moved and which way, and the drop-down reports the index it settled on. The label above follows all three."}
    }
}

fn drop_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let mut said: Option<String> = None;

    if let Some(v) = root.drop_slider(cx, ids!(level)).changed(actions) {
        said = Some(format!("master at {:.0}%", v * 100.0));
    }
    if let Some((index, on)) = root.drop_toggles(cx, ids!(filters)).toggled(actions) {
        let names = ["Stems", "Karaoke", "Key", "Tempo"];
        let name = names.get(index).copied().unwrap_or("?");
        said = Some(format!("{name} turned {}", if on { "on" } else { "off" }));
    }
    if let Some(i) = root.drop_down2(cx, ids!(source)).changed(actions) {
        let names = ["Deck A", "Deck B", "Microphone", "Line in"];
        said = Some(format!("source is {}", names.get(i).copied().unwrap_or("?")));
    }

    if let Some(text) = said {
        let label = root.label(cx, ids!(reported));
        if label.text() != text {
            label.set_text(cx, &text);
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/dropcontrols/overview",
    category: "Inputs",
    component: "DropControls",
    also: &["DropSlider", "DropToggles", "DropDown2"],
    name: "Overview",
    dsl: "DropControlsOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Drop controls

Three controls shaped for a crowded toolbar. Each is a small chip that opens a panel beneath itself: `DropSlider` opens a slider, `DropToggles` opens a set of switches, `DropDown2` opens a list.

**The reason they exist is that the drag is not in the bar.** A control you drag sideways inside a window's own chrome is competing with dragging the window itself. Whichever gesture wins, the other feels broken, and the usual repairs — hold a modifier, wait a moment, leave a dead zone — make the control worse to use rather than fixing anything. These move the drag one layer out: the bar keeps a plain click target that cannot be confused with anything, and the popover gets a full-sized control with room to aim at.

That is also why the chips are so small. `DropSlider` takes `min`, `max`, `default`, a `display_scale` for showing a fraction as a percentage, and a `suffix`; the number it shows is the value, and the popover is where you set it.

They report what changed rather than that something did: `changed` on the slider gives the value, `toggled` on the switches gives which one moved and which way, `changed` on the drop-down gives the index it settled on.

Their colours are literals rather than theme tokens, in the same way the charts and the data grid are — a caller wanting them to match a light page has to restate them.",
    subject: "level",
    feature: None,
    controls: &[],
    on_actions: Some(drop_actions),
}];
