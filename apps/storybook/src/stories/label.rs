//! The label story: one string in one style, its faces in a row, ellipsis
//! truncation across scripts, a text shader of its own and the styling
//! reference.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // The line over each case that says what it is set to.
    let Caption = Label{draw_text +: {color: theme.color_text_meta}}

    mod.stories.LabelOverview = StoryPage{
        StoryNote{text: "Which one to use, against the Text styles presets, TextFlow and RichTextEditor, is set out on the Docs tab."}

        StoryHeading{text: "One label, under the controls"}
        StoryNote{text: "One label; the controls change its text, size and colour."}
        StoryRow{
            subject := Label{text: "Hello"}
        }

        StoryHeading{text: "Faces"}
        StoryNote{text: "Label draws its text in one colour. LabelGradientX blends color into color_2 from left to right, and LabelGradientY from top to bottom; both start at red and end at yellow unless given colours of their own. TextBox is a label that fills its parent's width and wraps, in the theme's body size."}
        StoryRow{
            spacing: theme.space_3
            Label{text: "Label"}
            LabelGradientX{text: "LabelGradientX"}
            LabelGradientX{text: "LabelGradientX" draw_text +: {color: #0ff text_style +: {font_size: 20}}}
            LabelGradientY{text: "LabelGradientY"}
            LabelGradientY{text: "LabelGradientY" draw_text +: {color: #0ff text_style +: {font_size: 20}}}
        }
        TextBox{
            text: "Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo. Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit, sed quia consequuntur magni dolores eos qui ratione voluptatem sequi nesciunt."
        }

        StoryHeading{text: "Truncation"}
        StoryNote{text: "max_lines caps the lines a label may take, and text_overflow: Ellipsis ends the last of them with an ellipsis. The ellipsis lands between whole characters whatever their length in bytes, so emoji, CJK and mixed scripts truncate as cleanly as plain Latin. Resize the window to watch the truncation point move."}
        Caption{text: "one line, Fill width"}
        Label{
            width: Fill
            max_lines: 1
            text_overflow: Ellipsis
            text: "This is a very long label text that should be truncated with an ellipsis character at the end of the line when it overflows"
        }
        Caption{text: "two lines at most, Fill width"}
        Label{
            width: Fill
            max_lines: 2
            text_overflow: Ellipsis
            text: "This is a longer piece of text that should wrap to multiple lines. When it exceeds two lines, it should be truncated with an ellipsis character appended to the end of the second line. Try resizing the window to watch the wrapping and truncation adapt dynamically."
        }
        Caption{text: "a TextBox, three lines at most"}
        TextBox{
            max_lines: 3
            text_overflow: Ellipsis
            text: "Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo. Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit, sed quia consequuntur magni dolores eos qui ratione voluptatem sequi nesciunt."
        }
        Caption{text: "Fit width, capped at half the parent"}
        Label{
            width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.5}}
            max_lines: 1
            text_overflow: Ellipsis
            text: "This label uses Fit width with a relative max bound of 50%. Short text grows naturally, but long text like this gets truncated with an ellipsis once it hits the maximum width."
        }
        Caption{text: "emoji, one line"}
        Label{
            width: Fill
            max_lines: 1
            text_overflow: Ellipsis
            text: "Stars \u{2B50}\u{2B50}\u{2B50} and rockets \u{1F680}\u{1F680}\u{1F680} and flags \u{1F3C1}\u{1F3C1}\u{1F3C1} and more emoji to overflow the line boundary"
        }
        Caption{text: "only emoji, capped at half the parent"}
        Label{
            width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.5}}
            max_lines: 1
            text_overflow: Ellipsis
            text: "\u{1F600}\u{1F601}\u{1F602}\u{1F603}\u{1F604}\u{1F605}\u{1F606}\u{1F607}\u{1F608}\u{1F609}\u{1F60A}\u{1F60B}\u{1F60C}\u{1F60D}\u{1F60E}\u{1F60F}\u{1F610}\u{1F611}\u{1F612}\u{1F613}"
        }
        Caption{text: "CJK, capped at six tenths of the parent"}
        Label{
            width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.6}}
            max_lines: 1
            text_overflow: Ellipsis
            text: "\u{6587}\u{5B57}\u{306E}\u{30C6}\u{30B9}\u{30C8}\u{3067}\u{3059}\u{3002}\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{6587}\u{7AE0}\u{304C}\u{9577}\u{3059}\u{304E}\u{308B}\u{3068}\u{7701}\u{7565}\u{8A18}\u{53F7}\u{304C}\u{8868}\u{793A}\u{3055}\u{308C}\u{307E}\u{3059}"
        }
        Caption{text: "Latin, Cyrillic and emoji, two lines at most"}
        Label{
            width: Fill
            max_lines: 2
            text_overflow: Ellipsis
            text: "Hello \u{041F}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{1F44B} world! Multi-script text with various byte lengths per character should wrap and truncate cleanly across line boundaries when the window is resized."
        }

        StoryHeading{text: "Text that fits"}
        StoryNote{text: "A label short enough for its lines gets no ellipsis at all: the cap only ever shortens text that would run past it."}
        Label{
            width: Fill
            max_lines: 1
            text_overflow: Ellipsis
            text: "Short text"
        }

        StoryHeading{text: "A shader of its own"}
        StoryNote{text: "draw_text takes a get_color of its own, so the ink can be worked out per pixel. This one fades the theme's accent to clear from left to right across the glyphs."}
        Label{
            draw_text +: {
                get_color: fn() -> vec4 {
                    return mix(theme.color_makepad #0000 self.pos.x)
                }
                color: theme.color_makepad
                text_style +: {
                    font_size: 40.
                }
            }
            text: "OR EVEN SOME PIXELSHADERS"
        }

        StoryHeading{text: "Styling reference"}
        StoryNote{text: "draw_text.color is the ink, and draw_text.text_style carries the font, font_size and line_spacing."}
        Label{
            draw_text +: {
                color: #0ff
                text_style +: {
                    font_size: 20.
                    line_spacing: 1.4
                }
            }
            text: "You can style text using colors and fonts"
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "text/label/overview",
    category: "Text",
    component: "Label",
    also: &["TextBox"],
    name: "Overview",
    dsl: "LabelOverview",
    added: "2026-04-06",
    tags: &["controls", "ported"],
    doc: "# Label\n\nOne string in one style. `text` is the string, `draw_text.color` and `draw_text.text_style` are the style, and the label is as wide as its ink unless its walk says otherwise.\n\n## Which one to use\n\n| Want | Use |\n|---|---|\n| one string, styled by hand | `Label` |\n| a size that comes from the theme's type scale | a preset on Text styles: `Text`, `TextSmall`, `TextLarge`, `TextStrong`, `TextMuted`, `TextCode` |\n| a heading | `H1` to `H6`, on Text styles |\n| copy that wraps to its parent's width | `P` or `TextBox` |\n| a run that changes style part way through | `TextFlow`, or `Html` and `Markdown`, which are built on it |\n| text that is edited with marks in it | `RichTextEditor` |\n\n## Truncation\n\n`max_lines` caps how many lines a label may take, and `text_overflow: Ellipsis` ends the last of them with an ellipsis. The page walks it across emoji, CJK and mixed scripts, where a character can be up to four bytes long, and shows that a label short enough to fit gets no ellipsis at all.",
    subject: "",
    feature: None,
    controls: &[
            Control { label: "Text", target: "subject", kind: ControlKind::Text { prop: "text", default: "Hello" } },
            Control { label: "Size", target: "subject", kind: ControlKind::Number { prop: "draw_text.text_style.font_size", min: 6., max: 48., step: 0.5, default: 10. } },
            Control { label: "Colour", target: "subject", kind: ControlKind::Color { prop: "draw_text.color", default: 0xFFFFFFAA } },
        ],
    on_actions: None,
}];
