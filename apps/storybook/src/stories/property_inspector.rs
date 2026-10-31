//! The property inspector story: an object's properties as a column of rows,
//! how a row decides what kind of editor it is, and the row controls the
//! panel is built from.
use crate::makepad_widgets::property_inspector::{Prop, PropertyInspectorWidgetRefExt};
use crate::makepad_widgets::fab_controls::{
    FabColorPickWidgetRefExt, FabKnobWidgetRefExt, FabValueInputWidgetRefExt,
};
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

    // The ground the dev panel itself stands on, read off the panel's own
    // table rather than off the app's theme: the knob is drawn to be read
    // against this and against nothing else, and it is the one ground on this
    // page that does not change when the page's theme does. A SolidView and
    // not a View with show_bg: a bare View's draw_bg paints nothing.
    let KnobGround = SolidView{
        width: Fit
        height: Fit
        flow: Right
        spacing: 6.
        padding: 8.
        align: Align{y: 1.0}
        draw_bg +: {color: mod.fab.color_area}
    }

    // The ground a header row and its matrix stand on. Nothing inside it
    // clips: the names are MEANT to cross their neighbours' boxes, and the
    // padding is the room the overflow wants at the end it leans over --
    // about three quarters of the longest name, both sides here because the
    // page shows both leans.
    let DiagGround = SolidView{
        width: Fit
        height: Fit
        flow: Down
        spacing: 0.
        padding: Inset{left: 44 right: 44 top: 8 bottom: 8}
        clip_x: false
        clip_y: false
        draw_bg +: {color: mod.fab.color_area}
    }

    let DiagRow = View{
        width: Fit
        height: Fit
        flow: Right
        spacing: 0.
        clip_x: false
        clip_y: false
    }

    let KnobRow = View{
        width: Fit
        height: Fit
        flow: Right
        spacing: 0.
    }

    mod.stories.PropertyInspectorOverview = StoryPage{
        StoryNote{text: "A panel of an object's properties: a name on the left, an editor on the right, and a heading over each family of them. Nothing here is declared row by row — the host hands over a list of properties and the panel decides what each one needs."}

        StoryHeading{text: "One object, five kinds of value"}
        StoryNote{text: "A number is a field you drag or type in, a colour is a swatch that opens a picker, a flag is a box, a named value with a list behind it is a menu, and anything else is text. The last row is read-only: it is shown and cannot be changed. Drag the radius, then watch the two reports beside the panel — the first follows the drag, the second fires once when you let go."}
        StoryRow{
            panel := PropertyInspector{width: 320.}
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1
                live := Label{text: "nothing moved yet"}
                written := Label{text: "nothing written yet" draw_text +: {color: theme.color_text_meta}}
                asked := Label{text: "nothing reset yet" draw_text +: {color: theme.color_text_meta}}
            }
        }

        StoryHeading{text: "The headings come out of the names"}
        StoryNote{text: "Nobody wrote the words draw_bg or layout here. A name with a dot in it puts the row under a heading of its own prefix and shows only the leaf, so a panel groups itself out of what reflection already reports. Properties with no prefix are the object's own and lead the panel with nothing written over them. Click a heading to fold its group away; the count stays, because the count is the reason to open it again."}

        StoryHeading{text: "Two number fields"}
        StoryNote{text: "The panel above uses the property-panel number field: a short row, the value right-anchored so the column lines up, three points of travel before a press becomes a drag, and a double-click that asks for the default back. Set wide_numbers and the rows take the roomier field instead — ordinary widget height, the library's own theme, a step arrow at each end. Which number control belongs where outside a panel is set out on the NumberField page."}
        StoryRow{
            wide := PropertyInspector{width: 320. wide_numbers: true}
        }

        StoryHeading{text: "The row controls"}
        StoryNote{text: "The rows are built from controls that also stand on their own: a numeric field you drag sideways instead of typing into, a colour swatch that opens its own picker, and the labels a row is made of. They are shaped for a dense column: a fixed row height, the label pinned left and the value right."}

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
        StoryNote{text: "Where the platform cannot pin the pointer, it is not pinned while you scrub. The field asks for it and the request is refused, so the cursor walks off across the screen on a long drag instead of staying where you pressed, and the log records the refusal. The value still follows correctly; it is the pointer that misbehaves."}

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

        StoryHeading{text: "The knob"}
        StoryNote{text: "A number on a dial, for where a row is too much room: a cell of a matrix. Press it and pull up for more or down for less. The whole range is a hundred and fifty points of travel whatever size the knob is drawn at, Shift slows the drag to a tenth, and sideways counts for nothing. A double click takes it back to nought. Once it has been pressed it has the keyboard, and then the arrows and the wheel step it; a knob that is merely under the pointer lets the wheel go by, so a panel full of them still scrolls. Here at nought, at half and at full, and one with a name over it."}
        StoryRow{
            KnobGround{
                knob_off := FabKnob{value: 0.0}
                knob_half := FabKnob{value: 50.0}
                knob_full := FabKnob{value: 100.0}
                knob_named := FabKnob{label: "mix" value: 35.0}
            }
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1
                turned := Label{text: "nothing turned yet"}
                settled := Label{text: "nothing settled yet" draw_text +: {color: theme.color_text_meta}}
            }
        }

        StoryHeading{text: "Nought is off"}
        StoryNote{text: "Most cells of a matrix stand at nought, so nought has to read as off from across the panel: the arc is out, the tick and the number go down to the muted ink, and the first part above nought lights a lamp on the stop. The knob is drawn from the panel's own table and never from the app's theme, so switching this page's theme changes the page and leaves the knobs exactly as they were."}

        StoryHeading{text: "Down to a matrix cell"}
        StoryNote{text: "The face is the biggest circle the box holds once the words have had their rows, so a knob takes the size of the cell it is put in. These are 28 points across, the narrowest column an eight-column matrix comes down to in the panel: first with the number under them, then square and bare, the way a cell looks when the headers carry the names and a tooltip carries the number."}
        StoryRow{
            KnobGround{
                FabKnob{width: 28 value: 0.0}
                FabKnob{width: 28 value: 50.0}
                FabKnob{width: 28 value: 100.0}
            }
            KnobGround{
                FabKnob{width: 28 height: 28 show_readout: false value: 0.0}
                FabKnob{width: 28 height: 28 show_readout: false value: 50.0}
                FabKnob{width: 28 height: 28 show_readout: false value: 100.0}
            }
            KnobGround{
                FabKnob{width: 28 height: 28 show_readout: false value: 50.0 enabled: false}
            }
        }

        StoryHeading{text: "Naming the columns"}
        StoryNote{text: "A column 26 points across cannot hold the word Omarchy, let alone Windows 2000, so the name is turned on its side and let out over its neighbours. That is safe because the names are parallel: at 45 degrees a pitch of 26 leaves 18 points between one name and the next across the line, and a line of the panel's small face is ten, so however long they get they never touch. Each name stands on the middle of its own column's bottom edge, right above the knob it names."}
        StoryNote{text: "Fall hangs the name over the LEFT and is the default: over a matrix that is the empty corner above the row names, so the last column is never cut in half by the panel's edge. Rise is the spreadsheet convention and hangs over the right instead, which a host has to leave room for. The ink is meant to leave its own box, so nothing between the label and the panel's own frame may clip."}
        StoryRow{
            DiagGround{
                DiagRow{
                    FabDiagonalLabel{width: 26 text: "Dark"}
                    FabDiagonalLabel{width: 26 text: "Omarchy"}
                    FabDiagonalLabel{width: 26 text: "macOS dark"}
                    FabDiagonalLabel{width: 26 text: "Windows 2000"}
                    FabDiagonalLabel{width: 26 text: "Android dark"}
                    FabDiagonalLabel{width: 26 text: "Black orange"}
                    FabDiagonalLabel{width: 26 text: "NeXTSTEP"}
                    FabDiagonalLabel{width: 26 text: "Skeleton"}
                }
                KnobRow{
                    FabKnob{width: 26 height: 26 show_readout: false value: 0.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 20.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 40.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 60.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 80.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 100.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 0.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 50.0}
                }
            }
        }
        StoryRow{
            DiagGround{
                DiagRow{
                    FabDiagonalLabel{width: 26 text: "Dark" lean: DiagonalLean.Rise}
                    FabDiagonalLabel{width: 26 text: "Omarchy" lean: DiagonalLean.Rise}
                    FabDiagonalLabel{width: 26 text: "macOS dark" lean: DiagonalLean.Rise}
                    FabDiagonalLabel{width: 26 text: "Windows 2000" lean: DiagonalLean.Rise}
                    FabDiagonalLabel{width: 26 text: "Android dark" lean: DiagonalLean.Rise}
                    FabDiagonalLabel{width: 26 text: "Black orange" lean: DiagonalLean.Rise}
                    FabDiagonalLabel{width: 26 text: "NeXTSTEP" lean: DiagonalLean.Rise}
                    FabDiagonalLabel{width: 26 text: "Skeleton" lean: DiagonalLean.Rise}
                }
                KnobRow{
                    FabKnob{width: 26 height: 26 show_readout: false value: 0.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 20.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 40.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 60.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 80.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 100.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 0.0}
                    FabKnob{width: 26 height: 26 show_readout: false value: 50.0}
                }
            }
        }

        StoryNote{text: "The same names over a roomier column. At a pitch of 44 the names stand 31 points apart across the line against a line of ten, so they are much further apart than they need to be — this is what a panel that has the width looks like, and about where a plain horizontal label starts to be the better answer."}
        StoryRow{
            DiagGround{
                DiagRow{
                    FabDiagonalLabel{width: 44 text: "Dark"}
                    FabDiagonalLabel{width: 44 text: "Omarchy"}
                    FabDiagonalLabel{width: 44 text: "macOS dark"}
                    FabDiagonalLabel{width: 44 text: "Windows 2000"}
                    FabDiagonalLabel{width: 44 text: "Android dark"}
                    FabDiagonalLabel{width: 44 text: "Black orange"}
                    FabDiagonalLabel{width: 44 text: "NeXTSTEP"}
                    FabDiagonalLabel{width: 44 text: "Skeleton"}
                }
                KnobRow{
                    FabKnob{width: 44 height: 26 show_readout: false value: 0.0}
                    FabKnob{width: 44 height: 26 show_readout: false value: 20.0}
                    FabKnob{width: 44 height: 26 show_readout: false value: 40.0}
                    FabKnob{width: 44 height: 26 show_readout: false value: 60.0}
                    FabKnob{width: 44 height: 26 show_readout: false value: 80.0}
                    FabKnob{width: 44 height: 26 show_readout: false value: 100.0}
                    FabKnob{width: 44 height: 26 show_readout: false value: 0.0}
                    FabKnob{width: 44 height: 26 show_readout: false value: 50.0}
                }
            }
        }

        StoryHeading{text: "Row labels"}
        StoryNote{text: "A heading, the ordinary label, a dimmed one and a small one."}
        StoryRow{
            Panel{
                FabHeaderLabel{text: "FabHeaderLabel"}
                FabLabel{text: "FabLabel"}
                FabLabelDim{text: "FabLabelDim"}
                FabLabelSmall{text: "FabLabelSmall"}
            }
        }

        StoryHeading{text: "Rows and the search well"}
        StoryNote{text: "FabSection is a heading row with a hand cursor, for a host that folds its group on a press. FabPropRow is the row an editor sits in: the name dimmed at a fixed width on the left, cut short with an ellipsis when it is too long, and whatever follows it on the right. FabSearch is the filter well at the top of a panel, a text field in the panel's own frame."}
        StoryRow{
            Panel{
                FabSearch{}
                FabSection{title +: {text: "Layout"}}
                FabPropRow{name +: {text: "flow"} FabLabel{text: "Down"}}
                FabPropRow{name +: {text: "spacing between children"} FabLabel{text: "8"}}
            }
        }
    }
}

/// The properties of some imaginary object, in the shape reflection hands
/// them over: a name, and the value as text.
fn subject_props() -> Vec<Prop> {
    vec![
        Prop::new("title", "Second chorus"),
        Prop::new("visible", "true"),
        Prop::new("draw_bg.color", "#3c5a8aff"),
        Prop::new("draw_bg.border_radius", "4").number(0.0, 24.0, 0.25),
        Prop::new("draw_bg.opacity", "0.85").number(0.0, 1.0, 0.005),
        Prop::new("layout.flow", "Down").choices(&["Right", "Down", "Overlay"]),
        Prop::new("layout.spacing", "8").number(0.0, 64.0, 0.5),
        Prop::new("layout.clip", "false"),
        Prop::new("uid", "0x8f21").read_only(),
    ]
}

fn property_inspector_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // There is nothing to declare in the DSL: a panel with no properties is
    // an empty box until a host feeds it, which is the call every real host
    // has to make.
    for id in [ids!(panel), ids!(wide)] {
        let inspector = root.property_inspector(cx, id);
        if inspector.count() == 0 {
            inspector.set_props(cx, subject_props());
        }
    }

    let inspector = root.property_inspector(cx, ids!(panel));
    // Two reports, not one: a host that writes to its document on every step
    // of a drag writes hundreds of times. Follow with the first, commit with
    // the second.
    if let Some((name, value)) = inspector.changed(actions) {
        root.label(cx, ids!(live))
            .set_text(cx, &format!("{name} is {value}"));
    }
    if let Some((name, value)) = inspector.committed(actions) {
        root.label(cx, ids!(written))
            .set_text(cx, &format!("wrote {name} = {value}"));
    }
    // The panel does not know what a default is, so a double-click on a
    // number asks the host for one and the host answers by pushing the
    // properties again.
    if let Some(name) = inspector.reset(actions) {
        root.label(cx, ids!(asked))
            .set_text(cx, &format!("{name} asked for its default back"));
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

    for (name, id) in [
        ("the first knob", ids!(knob_off)),
        ("the second knob", ids!(knob_half)),
        ("the third knob", ids!(knob_full)),
        ("mix", ids!(knob_named)),
    ] {
        let knob = root.fab_knob(cx, id);
        if let Some(v) = knob.changed(actions) {
            root.label(cx, ids!(turned))
                .set_text(cx, &format!("{name} is at {v:.0}"));
        }
        // The same two reports as the field above, and a third: a double
        // click says it was a reset as well as where it landed, for a host
        // that keeps a ledger of which cells count for anything.
        if let Some(v) = knob.ended(actions) {
            let how = if knob.was_reset(actions) { "reset" } else { "settled" };
            root.label(cx, ids!(settled))
                .set_text(cx, &format!("{name} {how} at {v:.0}"));
        }
    }
}

/// The page's one handler: the inspector panels, then the row controls.
fn inspector_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    property_inspector_actions(cx, root, actions);
    fab_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/property-inspector/overview",
    category: "Inputs",
    component: "PropertyInspector",
    also: &[
        "FabColorPick", "FabColorWheel", "FabLabel", "FabPaletteStrip", "FabValueInput", "Panel",
        "FabSection", "FabPropRow", "FabSearch", "FabKnob", "FabDiagonalLabel",
    ],
    name: "Overview",
    dsl: "PropertyInspectorOverview",
    added: "2026-09-10",
    tags: &["reflection", "panel"],
    doc: "# PropertyInspector

An object's properties as rows of name and editor, grouped under headings, each row taking the editor its value asks for.

## What you hand it

A list of properties, from Rust — there is nothing to declare in the DSL. A property is the shape reflection already produces: a **name**, a **value as text**, and the few things the host knows that the text cannot say.

```
Prop::new(\"visible\", \"true\")
Prop::new(\"draw_bg.color\", \"#3c5a8aff\")
Prop::new(\"draw_bg.border_radius\", \"4\").number(0.0, 24.0, 0.25)
Prop::new(\"layout.flow\", \"Down\").choices(&[\"Right\", \"Down\", \"Overlay\"])
Prop::new(\"uid\", \"0x8f21\").read_only()
```

**Text is the one channel, in both directions.** Five editors write five kinds of value, and a panel that modelled each one would need the host's type system inside it. The host parses `#3c5a8aff` or `12.5` back into whatever it actually keeps, which is where that knowledge lives.

## How a row picks its editor

| the value | the editor |
|---|---|
| `#3c5a8a`, `#3c5a8aff` | a swatch that opens a colour picker |
| anything that parses as a number | a field you drag or type in |
| `true` / `false` | a box |
| a value with `choices` behind it | a menu |
| anything else | a text field |
| a property marked `read_only` | shown, not edited |

A value cannot say what the *other* choices would have been, so only the host can turn a word into a menu, by listing them. A menu with nothing in it is text.

## Where the headings come from

A name with a dot puts its row under a heading of its own prefix and shows only the leaf, so `draw_bg.color` becomes **color** under **draw_bg**. A group gathers every property that names it however far apart they arrived; properties with no prefix lead the panel with no heading over them. Click a heading to fold the group away — it keeps its heading and its count.

## It reports; it never writes

A row reports and the host applies. The host owns undo, the clamp, and what a property means; a panel that wrote through would be a second author of the same state, and the two would disagree the first time a value was refused.

`changed` follows a gesture live, `committed` fires once when it finishes — that is where a document is written. `reset` is a double-click on a number asking for a default only the host knows.

## Two number fields

`FabValueInput` is the default because it is the property-panel field: a short fixed row, the number right-anchored so a column lines up, three points of travel before a press becomes a drag, and `Ended` at the commit point. `wide_numbers: true` swaps in `ValueInput` — ordinary widget height, the library's own theme, a step arrow at each end — for an inspector used as a form on a page. Which number control belongs where outside a panel is set out once, on NumberField. How many decimals a number shows belongs to the row template, not to the property: a column reads best sharing one precision.

## What it does not do

It does not scroll or recycle rows — put it in a scrolling parent. It does not open nested values: an inset is one row of text, and a host that wants four rows hands over four properties. It is not a form: validation, and where the keyboard goes after a refused submit, need to know the order the fields were meant to be filled in, and this panel's order is whatever the host handed over.

## The row controls

The controls the rows are made of, each usable on its own. They are shaped for a dense column of rows: a fixed row height, the label pinned left, the value right.

### The drag-numeric field

`FabValueInput` is a number you pull sideways rather than type into. `step` is the granularity per pixel of travel, `min` and `max` bound it, `precision` rounds the display, `suffix` carries the unit, `wrap` lets an angle come round again, and `show_fill` draws the value as a bar behind the number.

**A press is not yet a drag.** Pressing arms the field and it takes three points of travel before the number moves; release before that and it opens for keyboard entry instead. That single rule is what lets one control be both a slider and a text field with no mode to switch, and it is why a careless click cannot nudge a value.

**The pointer is not pinned on every platform.** A drag-numeric field normally holds the cursor still and lets the value run, so a long drag does not send the pointer off the edge of the screen. This one asks for that, and a backend that cannot do it answers `Not implemented on this platform`, twice per gesture, into the log. The value is unaffected: the pointer simply travels with your hand, and a long drag ends somewhere else on screen.

**It reports twice, and the difference matters.** `Changed` fires live, on every step of the drag; `Ended` fires once, when the gesture finishes. A host that writes to its document on `Changed` writes hundreds of times for one drag: follow with the first, commit with the second.

### The swatch

`FabColorPick` is a colour that opens its own picker over the page: a wheel, a strip of recent choices, and hex entry, which is what `FabColorWheel` and `FabPaletteStrip` are for. It reports the same way: live while you move inside it, and again when it closes.

### The knob

`FabKnob` is a number on a dial, for where a row is too much room: a cell of a matrix. It is 44 by 64 unless told otherwise and takes whatever box it is given, fixed or `Fill`; the face is the biggest circle that box holds once the two text rows are taken off its height, and it is drawn to stay legible 28 points across. `label` is the name over the face, and an empty one takes its row away too. `show_readout` is the number under it. `min`, `max`, `step`, `big_step`, `precision`, `unit`, `value` and `enabled` mean what they mean on the panel's slider, so a host treats the two alike.

**Pull up for more.** A press takes the pointer and keeps it until the release; the value is how far the pointer has come since, not where it is, because a dial 28 points across has no room for the other law. `drag_travel` is the travel that covers the whole range, 150 points, and it is the same at every size. Shift is a tenth of the speed, sideways counts for nothing, and three points of travel pass before a press becomes a drag, so a click that was only meant to select a knob does not nudge it.

**A double click is the reset.** It goes to nought, as the slider's name does when it is clicked; a knob in a matrix has no name to click.

**The wheel is for the knob that has the keyboard.** A press gives a knob the keyboard, and then the arrows step it, Shift takes `big_step`, Home and End go to the stops, and the wheel steps it a notch at a time. A knob that is merely under the pointer lets the wheel go by: the panel these sit in scrolls and is a wall of them. `wheel_on_hover: true` is for a host whose knobs stand somewhere that does not.

**It reports the way the rest of the kit does.** `Changed` follows a gesture live and `Ended` fires once when it is over: at the release of a drag, at the release of a held arrow, when the wheel has been still for a third of a second, and at once for a double click, which also says `Reset`. A press that turned nothing commits nothing. `set_value` and `set_value_and_readout` say nothing at all, so a host can fill a matrix without hearing its own numbers back.

**Nought reads as off.** The arc is out and the tick and the number are dimmed, because most cells of a matrix stand at nought. Every colour comes from the panel's own table, so the knob looks the same under every theme the app can wear.

### The diagonal header

`FabDiagonalLabel` is a name written across the corner of the box it names, for a column too narrow to hold it flat. A matrix of knobs comes down to about 26 points a column and a theme's name runs to two and a half times that, so the name is turned — `angle`, 45 degrees by default — and let out over its neighbours. Parallel names never collide: at 45 degrees a pitch of 26 puts 18 points between one and the next across the line, and a line of the panel's small face is ten.

**One end always stands on its own column.** The anchor is the box's bottom centre in both leans, so the name is over the thing it names whatever length it is. `lean: Fall` — the default — *ends* there, having begun up and to the left, and hangs over the LEFT; over a matrix that is the empty corner above the row names, so the last column is never cut in half by the panel's edge. `lean: Rise` *starts* there and rises to the upper right, the spreadsheet convention, and the host owes its last column that much room.

**Nothing may clip it.** The ink leaves the widget's own box on purpose, which is why the widget opens no turtle of its own; the row it stands in needs `clip_x: false`, and the ground the overflow lands on has to be inside whatever does clip. A header row that looks like it is losing letters is a clip somewhere above it, not the label.

**How tall the row has to be.** `text_width x sin(angle) + line_height x cos(angle)` — the name along the hypotenuse plus its line across it. `diagonal_row_height` in Rust is that sum, and `FabDiagonalLabel::row_height_for(cx, longest)` shapes a name and applies it, so a host fixes its header once at the longest name it will ever write. At 45 degrees with the panel's small face the longest theme name the library ships wants 56.5 points, which is where the template's default of 60 comes from.

**It is a label.** No hit, no focus, no actions. `set_text` holds the name and asks for a frame, and says nothing when the name has not changed, because a matrix writes every header into a fixed slot on every draw.

### The labels

`FabHeaderLabel`, `FabLabel`, `FabLabelDim` and `FabLabelSmall` are the text a row is made of: a group heading, the ordinary label, a dimmed one and a small one.

### Rows and the search well

`FabSection` is a heading row, `title` inside it, with a hand cursor for a host that folds its group on a press; the row itself folds nothing. `FabPropRow` is the row an editor sits in, at the panel's row height: `name` is a dimmed label at a fixed width, one line with an ellipsis, and the children that follow it take the rest of the row. `FabSearch` is the filter well, a `TextInput` named `input` inside the panel's own frame.",
    subject: "panel",
    feature: None,
    controls: &[],
    on_actions: Some(inspector_page_actions),
}];
