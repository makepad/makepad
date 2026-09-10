//! The placeholder stories: the three shapes, the three animations and
//! every preset, and one placeholder under the controls.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Caption = Label{
        draw_text +: {color: theme.color_on_surface_variant}
    }

    let Column = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_2
    }

    mod.stories.PlaceholderOverview = StoryPage{
        StoryHeading{text: "Shapes"}
        StoryNote{text: "A rect, a circle, and a stack of text lines whose last line is shorter."}
        StoryHeading{text: "One stack of lines, under the controls"}
        StoryNote{text: "One stack of lines; the line count, the animation and the translucency come from the controls."}
        StoryRow{
            subject := ContentPlaceholder{shape: PlaceholderShape.Lines lines: 3 width: 320. height: Fit}
        }
        StoryRow{
            spacing: theme.space_3
            ContentPlaceholder{width: 160. height: 12.}
            PlaceholderCircle{}
            ContentPlaceholder{shape: PlaceholderShape.Lines lines: 3 width: 220. height: Fit}
            ContentPlaceholder{shape: PlaceholderShape.Lines lines: 2 last_line_width: 0.35 line_height: 14. width: 180. height: Fit}
        }

        StoryHeading{text: "Animations"}
        StoryNote{text: "Wave sweeps a highlight across; Pulse breathes the whole shape; Static stays still, as does anything with reduced_motion. The last one is translucent."}
        StoryRow{
            spacing: theme.space_3
            Column{
                ContentPlaceholder{width: 140. height: 40. animation: Wave}
                Caption{text: "wave"}
            }
            Column{
                ContentPlaceholder{width: 140. height: 40. animation: Pulse}
                Caption{text: "pulse"}
            }
            Column{
                ContentPlaceholder{width: 140. height: 40. animation: Static}
                Caption{text: "static"}
            }
            Column{
                ContentPlaceholder{width: 140. height: 40. reduced_motion: true}
                Caption{text: "reduced motion"}
            }
            Column{
                ContentPlaceholder{width: 140. height: 40. translucent: true}
                Caption{text: "translucent"}
            }
        }

        StoryHeading{text: "Presets"}
        StoryNote{text: "Text, circle, image, button and input."}
        StoryRow{
            spacing: theme.space_3
            PlaceholderText{width: 160.}
            PlaceholderCircle{}
            PlaceholderImage{width: 160. height: 90.}
            PlaceholderButton{}
            PlaceholderInput{width: 160.}
        }
        StoryNote{text: "Paragraph, row and table."}
        StoryRow{
            spacing: theme.space_3
            align: Align{x: 0. y: 0.}
            PlaceholderParagraph{width: 300.}
            PlaceholderRow{width: 300.}
        }
        StoryRow{
            PlaceholderTable{width: 480.}
        }
        StoryNote{text: "Card."}
        StoryRow{
            PlaceholderCard{}
        }
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/placeholder/overview",
        category: "Feedback",
        component: "Placeholder",
        also: &["ContentPlaceholder", "PlaceholderButton", "PlaceholderCard", "PlaceholderCircle", "PlaceholderImage", "PlaceholderInput", "PlaceholderParagraph", "PlaceholderRow", "PlaceholderTable", "PlaceholderText"],
        name: "Overview",
        dsl: "PlaceholderOverview",
        added: "2026-09-05",
        tags: &["controls", "new"],
        doc: "# Placeholder\n\nA `ContentPlaceholder` is the grey shape that stands in for content that has not arrived: a rect, a circle, or a stack of text lines (`lines`, `line_height`, `line_gap`, `last_line_width`). It shimmers (`animation: Wave`), breathes (`Pulse`) or stays still (`Static`, or `reduced_motion: true`), and can be `translucent` over content that is still partly there.\n\nThe presets compose it into the things it usually stands in for: `PlaceholderText`, `PlaceholderCircle`, `PlaceholderImage`, `PlaceholderButton`, `PlaceholderInput`, `PlaceholderParagraph`, `PlaceholderRow`, `PlaceholderCard` and `PlaceholderTable`.\n\nOnly the highlight reads the pass clock, so a page of static placeholders costs the window nothing.",
        subject: "",
        feature: None,
        controls: &[
            Control { label: "Lines", target: "subject", kind: ControlKind::Number { prop: "lines", min: 1., max: 8., step: 1., default: 3. } },
            Control {
                label: "Animation",
                target: "subject",
                kind: ControlKind::Choice { prop: "animation", options: &["Wave", "Pulse", "Static"], default: 0 },
            },
            Control { label: "Translucent", target: "subject", kind: ControlKind::Bool { prop: "translucent", default: false } },
            Control { label: "Last line", target: "subject", kind: ControlKind::Number { prop: "last_line_width", min: 0.1, max: 1., step: 0.05, default: 0.6 } },
        ],
        on_actions: None,
    },
];
