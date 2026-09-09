//! The fab controls story: a numeric field you scrub rather than type in,
//! and the swatch that opens a picker.
use crate::makepad_widgets::fab_controls::{FabColorPickWidgetRefExt, FabValueInputWidgetRefExt};
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Panel = View{
        width: 300.
        height: Fit
        flow: Down
        spacing: 2.
        padding: theme.mspace_2
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_low}
    }

    mod.stories.FabControlsOverview = StoryPage{
        StoryNote{text: "The controls a property panel is made of: a numeric field you drag sideways instead of typing into, and a colour swatch that opens its own picker. Both come from the node editor and are built to sit in a dense column of rows."}

        StoryHeading{text: "Drag sideways"}
        StoryNote{text: "Press one and pull left or right. The value follows the pointer at the rate its step says, and the label stays put so a column of them reads as a table. Give it a min and a max and it stops at them; give it a suffix and the unit rides along with the number."}
        StoryRow{
            Panel{
                plain := FabValueInput{label: "Opacity" value: 0.65 min: 0.0 max: 1.0 step: 0.005}
                sized := FabValueInput{label: "Radius" value: 12.0 min: 0.0 max: 64.0 step: 0.25 precision: 1 suffix: "px"}
                angled := FabValueInput{label: "Angle" value: 45.0 min: 0.0 max: 360.0 step: 1.0 precision: 0 suffix: "deg" wrap: true}
                filled := FabValueInput{label: "Mix" value: 0.4 min: 0.0 max: 1.0 step: 0.005 show_fill: true}
            }
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1
                reported := Label{text: "nothing dragged yet"}
                committed := Label{text: "nothing committed yet" draw_text +: {color: theme.color_text_meta}}
            }
        }

        StoryHeading{text: "A press is not yet a drag"}
        StoryNote{text: "Pressing arms the field; it takes three points of travel before the number starts to move. Release before that and it opens for typing instead — try both on the Angle row. That one rule is what lets the same control be both a slider and a text field without a mode switch, and it is why a careless click does not nudge the value."}
        StoryNote{text: "On this platform the pointer is not pinned while you scrub. The field asks for it and the request is not implemented on Windows, so the cursor walks off across the screen on a long drag instead of staying where you pressed, and the log fills with the refusal. The value still follows correctly; it is the pointer that misbehaves."}

        StoryHeading{text: "The swatch"}
        StoryNote{text: "A colour is a swatch that opens a picker over the page: a wheel, a strip of recent choices, and hex entry. It reports as you move inside it and again when it closes, so a host can follow a drag live and still know when to write the change down."}
        StoryRow{
            Panel{
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    FabLabel{text: "Tint"}
                    swatch := FabColorPick{}
                }
            }
            picked := Label{text: "no colour chosen"}
        }
    }
}

fn fab_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for (name, id) in [
        ("Opacity", ids!(plain)),
        ("Radius", ids!(sized)),
        ("Angle", ids!(angled)),
        ("Mix", ids!(filled)),
    ] {
        let field = root.fab_value_input(cx, id);
        if let Some(v) = field.changed(actions) {
            root.label(cx, ids!(reported))
                .set_text(cx, &format!("{name} is at {v:.3}"));
        }
        // Two reports, not one: a host that writes to a document on every
        // pixel of a drag writes hundreds of times. Changed is for following,
        // Ended is for committing.
        if let Some(v) = field.ended(actions) {
            root.label(cx, ids!(committed))
                .set_text(cx, &format!("committed {name} at {v:.3}"));
        }
    }

    if let Some(c) = root.fab_color_pick(cx, ids!(swatch)).changed(actions) {
        root.label(cx, ids!(picked)).set_text(
            cx,
            &format!("r {:.2}  g {:.2}  b {:.2}  a {:.2}", c.x, c.y, c.z, c.w),
        );
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/fabcontrols/overview",
    category: "Inputs",
    component: "FabControls",
    also: &[
        "FabValueInput",
        "FabColorPick",
        "FabColorWheel",
        "FabPaletteStrip",
        "FabLabel",
    ],
    name: "Overview",
    dsl: "FabControlsOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Fab controls

The controls a property panel is made of. They come from the node editor and are shaped for a dense column of rows: a fixed row height, the label pinned left, the value right.

## The drag-numeric field

`FabValueInput` is a number you pull sideways rather than type into. `step` is the granularity per pixel of travel, `min` and `max` bound it, `precision` rounds the display, `suffix` carries the unit, `wrap` lets an angle come round again, and `show_fill` draws the value as a bar behind the number.

**A press is not yet a drag.** Pressing arms the field and it takes three points of travel before the number moves; release before that and it opens for keyboard entry instead. That single rule is what lets one control be both a slider and a text field with no mode to switch, and it is why a careless click cannot nudge a value.

**The pointer is not pinned on every platform.** A drag-numeric field normally holds the cursor still and lets the value run, so a long drag does not send the pointer off the edge of the screen. This one asks for that and the Windows backend answers `Not implemented on this platform`, twice per gesture, into the log. The value is unaffected — the pointer simply travels with your hand, and a long drag ends somewhere else on screen.

**It reports twice, and the difference matters.** `Changed` fires live, on every step of the drag; `Ended` fires once, when the gesture finishes. A host that writes to its document on `Changed` writes hundreds of times for one drag — follow with the first, commit with the second.

## The swatch

`FabColorPick` is a colour that opens its own picker over the page — a wheel, a strip of recent choices, and hex entry, which is what `FabColorWheel` and `FabPaletteStrip` are for. It reports the same way: live while you move inside it, and again when it closes.",
    subject: "plain",
    feature: None,
    controls: &[],
    on_actions: Some(fab_actions),
}];
