//! Dividers: a rule between two things, with a label, a style and an inset.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.DividerOverview = StoryPage{
        StoryHeading{text: "Orientations"}
        StoryNote{text: "A horizontal rule fills its parent's width; the vertical one stands as tall as the row it is in."}
        Divider{}
        StoryRow{
            height: 40.
            Label{text: "Left"}
            vertical := DividerVertical{}
            Label{text: "Middle"}
            vertical_strong := DividerVertical{appearance: DividerAppearance.Strong}
            Label{text: "Right"}
        }

        StoryHeading{text: "Labels"}
        StoryNote{text: "A label splits the rule in two, at the start, the centre or the end."}
        Divider{text: "Start", label_at: DividerLabelAt.Start}
        labelled := DividerLabelled{}
        Divider{text: "End", label_at: DividerLabelAt.End}

        StoryHeading{text: "Styles"}
        Divider{style: DividerStyle.Solid, text: "solid"}
        Divider{style: DividerStyle.Dashed, text: "dashed"}
        Divider{style: DividerStyle.Dotted, text: "dotted"}

        StoryHeading{text: "Appearances"}
        Divider{appearance: DividerAppearance.Subtle, text: "subtle"}
        Divider{appearance: DividerAppearance.Strong, text: "strong"}
        Divider{appearance: DividerAppearance.Brand, text: "brand"}

        StoryHeading{text: "Insets"}
        StoryNote{text: "An inset keeps the rule off the leading edge, or off both, so list separators line up with the text after a leading column."}
        Divider{inset: DividerInset.None, appearance: DividerAppearance.Strong}
        Divider{inset: DividerInset.Start, appearance: DividerAppearance.Strong}
        Divider{inset: DividerInset.Middle, appearance: DividerAppearance.Strong}
    

        StoryHeading{text: "One divider, under the controls"}
        StoryNote{text: "One divider. The controls write its label, where the label sits, its style, weight and inset."}
        subject := Divider{text: "or"}
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "layout/divider/overview",
        category: "Layout",
        component: "Divider",
        also: &["DividerLabelled", "DividerVertical"],
        name: "Overview",
        dsl: "DividerOverview",
        added: "2026-09-05",
        tags: &["controls", "new"],
        doc: "# Divider\n\nA hairline between two things. `Divider` lies across its parent, `DividerVertical` stands as tall as its parent lets it, `DividerLabelled` carries an \"or\" in the middle. The label can sit at the start, the centre or the end and splits the rule into two segments; the style is solid, dashed or dotted; the appearance is subtle, strong or brand; the inset keeps the rule off the leading edge or off both.\n\n`Hr` and `Vr` keep drawing the theme's bevelled groove and are untouched.",
        subject: "labelled",
        feature: None,
        controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "or" } },
            Control {
                label: "Label at",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "label_at",
                    options: &["DividerLabelAt.Start", "DividerLabelAt.Center", "DividerLabelAt.End"],
                    default: 1,
                },
            },
            Control {
                label: "Style",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "style",
                    options: &["DividerStyle.Solid", "DividerStyle.Dashed", "DividerStyle.Dotted"],
                    default: 0,
                },
            },
            Control {
                label: "Appearance",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "appearance",
                    options: &["DividerAppearance.Subtle", "DividerAppearance.Strong", "DividerAppearance.Brand"],
                    default: 0,
                },
            },
            Control {
                label: "Inset",
                target: "subject",
                kind: ControlKind::Choice {
                    prop: "inset",
                    options: &["DividerInset.None", "DividerInset.Start", "DividerInset.Middle"],
                    default: 0,
                },
            },
            Control { label: "Thickness", target: "subject", kind: ControlKind::Number { prop: "thickness", min: 0.5, max: 8., step: 0.5, default: 1. } },
        ],
        on_actions: None,
    },
];
