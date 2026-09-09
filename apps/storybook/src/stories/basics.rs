//! Controlled stories for the core widgets: one instance, every declared
//! property live in the controls panel.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ButtonBasic = StoryPage{
        StoryNote{text: "One button. Every control on the right writes into it."}
        StoryRow{
            subject := Button{text: "Button"}
        }
        StoryRow{
            clicks := Label{text: "not clicked yet"}
        }
    }

    mod.stories.CheckBoxBasic = StoryPage{
        StoryNote{text: "One checkbox and one toggle, driven from the controls."}
        StoryRow{
            subject := CheckBox{text: "Option"}
            toggle := Toggle{text: "Switch"}
        }
    }

    mod.stories.SliderBasic = StoryPage{
        StoryNote{text: "One slider; its range and step come from the controls."}
        StoryRow{
            subject := Slider{width: 240. text: "Amount"}
        }
    }

    mod.stories.TextInputBasic = StoryPage{
        StoryNote{text: "One text input with a placeholder."}
        StoryRow{
            subject := TextInput{width: 240. empty_text: "Type here"}
        }
    }

    mod.stories.LabelBasic = StoryPage{
        StoryNote{text: "One label; the controls change its text and size."}
        StoryRow{
            subject := Label{text: "Hello"}
        }
    }
}

fn button_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(subject)).clicked(actions) {
        let n = crate::stories::bump(live_id!(button_basic));
        root.label(cx, ids!(clicks)).set_text(cx, &format!("clicked {n} time{}", if n == 1 { "" } else { "s" }));
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "actions/button/basic",
        category: "Actions",
        component: "Button",
        also: &[],
        name: "Basic",
        dsl: "ButtonBasic",
        added: "2025-05-06",
        tags: &["controls"],
        doc: "# Button\n\nA button raises `Clicked` when pressed and released over it, and `Pressed`/`Released` around that. The label and the icon sit inside a face whose fill, border and text mix towards their hover, down, focus and disabled colours through the animator.\n\nUse the controls to change the label, the corner radius, the fill and the disabled state.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Button" } },
            Control { label: "Corner radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 16., step: 0.5, default: 2.5 } },
            Control { label: "Fill", target: "subject", kind: ControlKind::Color { prop: "draw_bg.color", default: 0xFFFFFF88 } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(button_actions),
    },
    Story {
        key: "inputs/checkbox/basic",
        category: "Inputs",
        component: "CheckBox",
        also: &["Toggle"],
        name: "Basic",
        dsl: "CheckBoxBasic",
        added: "2025-05-06",
        tags: &["controls"],
        doc: "# CheckBox\n\nA checkbox carries one boolean and raises `Changed` when it flips. The toggle is the same widget with a pill face and a sliding knob.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Option" } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: None,
    },
    Story {
        key: "inputs/slider/basic",
        category: "Inputs",
        component: "Slider",
        also: &[],
        name: "Basic",
        dsl: "SliderBasic",
        added: "2025-05-06",
        tags: &["controls"],
        doc: "# Slider\n\nA slider maps a drag along its track to a value between `min` and `max`, snapping to `step` when one is set, and raises `Slided` while dragging.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Amount" } },
            Control { label: "Maximum", target: "subject", kind: ControlKind::Number { prop: "max", min: 1., max: 1000., step: 1., default: 1. } },
            Control { label: "Step", target: "subject", kind: ControlKind::Number { prop: "step", min: 0., max: 10., step: 0.1, default: 0. } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: None,
    },
    Story {
        key: "inputs/textinput/basic",
        category: "Inputs",
        component: "TextInput",
        also: &[],
        name: "Basic",
        dsl: "TextInputBasic",
        added: "2025-05-06",
        tags: &["controls"],
        doc: "# TextInput\n\nA text field with selection, clipboard, undo and an input-method path, raising `Changed` on every edit and `Returned` on enter.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Placeholder", target: "subject", kind: ControlKind::Text { prop: "empty_text", default: "Type here" } },
            Control { label: "Password", target: "subject", kind: ControlKind::Bool { prop: "is_password", default: false } },
            Control { label: "Read only", target: "subject", kind: ControlKind::Bool { prop: "is_read_only", default: false } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: None,
    },
    Story {
        key: "text/label/basic",
        category: "Text",
        component: "Label",
        also: &[],
        name: "Basic",
        dsl: "LabelBasic",
        added: "2025-05-06",
        tags: &["controls"],
        doc: "# Label\n\nStatic text with wrapping, a line limit and an overflow mode.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Text", target: "subject", kind: ControlKind::Text { prop: "text", default: "Hello" } },
            Control { label: "Size", target: "subject", kind: ControlKind::Number { prop: "draw_text.text_style.font_size", min: 6., max: 48., step: 0.5, default: 10. } },
            Control { label: "Colour", target: "subject", kind: ControlKind::Color { prop: "draw_text.color", default: 0xFFFFFFAA } },
        ],
        on_actions: None,
    },
];
