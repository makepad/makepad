//! The property inspector story: an object's properties as a column of rows,
//! and how a row decides what kind of editor it is.
use crate::makepad_widgets::drag_number::{Prop, PropertyInspectorWidgetRefExt};
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

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

        StoryHeading{text: "Two number fields, and no third one"}
        StoryNote{text: "The panel above uses the property-panel number field: a short row, the value right-anchored so the column lines up, three points of travel before a press becomes a drag, and a double-click that asks for the default back. Set wide_numbers and the rows take the roomier field instead — ordinary widget height, the library's own theme, a step arrow at each end. Both already existed; this panel writes neither."}
        StoryRow{
            wide := PropertyInspector{width: 320. wide_numbers: true}
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

pub const STORIES: &[Story] = &[Story {
    key: "inputs/property-inspector/overview",
    category: "Inputs",
    component: "PropertyInspector",
    also: &[],
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

## Two number fields, and no third

`FabValueInput` is the default because it is the property-panel field: a short fixed row, the number right-anchored so a column lines up, three points of travel before a press becomes a drag, and `Ended` at the commit point. `wide_numbers: true` swaps in `ValueInput` — ordinary widget height, the library's own theme, a step arrow at each end — for an inspector used as a form on a page. Both already existed. How many decimals a number shows belongs to the row template, not to the property: a column reads best sharing one precision.

## What it does not do

It does not scroll or recycle rows — put it in a scrolling parent. It does not open nested values: an inset is one row of text, and a host that wants four rows hands over four properties. It is not a form: validation, and where the keyboard goes after a refused submit, need to know the order the fields were meant to be filled in, and this panel's order is whatever the host handed over.",
    subject: "panel",
    feature: None,
    controls: &[],
    on_actions: Some(property_inspector_actions),
}];
