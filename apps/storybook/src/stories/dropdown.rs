//! The drop down stories: two combo boxes and the drop down ladder, ported
//! from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.DropDownOverview = StoryPage{
        H4{text: "ComboBox — type to filter, Enter commits the highlighted match"}
        combo := ComboBox{
            width: 220
            labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
        }

        Hr{}
        H4{text: "ComboBox — long list (40 rows, scrolls past 12)"}
        combo_long := ComboBox{
            width: 220
            labels: [
                "amber" "azure" "basalt" "beacon" "bramble" "cinder" "citrine" "cobalt"
                "coral" "cypress" "dahlia" "dusk" "ember" "fathom" "fennel" "flint"
                "garnet" "gossamer" "harbor" "indigo" "juniper" "kestrel" "lantern" "lichen"
                "marble" "meadow" "nimbus" "onyx" "opal" "pewter" "quarry" "quill"
                "russet" "saffron" "slate" "thistle" "umber" "verdant" "willow" "zephyr"
            ]
        }

        Hr{}
        H4{text: "Standard"}
        dropdown := DropDown{
            labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
        }

        Hr{}
        H4{text: "Standard, disabled"}
        dropdown_disabled := DropDown{
            labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }

        Hr{}
        H4{text: "DropDownFlat"}
        dropdown_flat := DropDownFlat{
            labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
        }

        Hr{}
        H4{text: "DropDownGradientX"}
        dropdown_gradient_x := DropDownGradientX{
            labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
        }

        Hr{}
        H4{text: "DropDownGradientY"}
        dropdown_gradient_y := DropDownGradientY{
            labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/dropdown/overview",
    category: "Inputs",
    component: "DropDown",
    also: &["ComboBox"],
    name: "Overview",
    dsl: "DropDownOverview",
    added: "2026-08-23",
    tags: &["ported"],
    doc: "# DropDown\n\nDropdowns allow selecting from a list of options.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];
