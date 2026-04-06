use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoLabel = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Label\n\nLabels display text content."}
        }
        demos +: {
            H4{text: "Standard"}
            Label{text: "Default single line text"}

            Hr{}
            H4{text: "LabelGradientX"}
            LabelGradientX{text: "LabelGradientX"}
            LabelGradientX{
                draw_text +: {
                    color: #0ff
                    text_style +: {
                        font_size: 20
                    }
                }
                text: "LabelGradientX"
            }

            Hr{}
            H4{text: "LabelGradientY"}
            LabelGradientY{text: "LabelGradientY"}
            LabelGradientY{
                draw_text +: {
                    color: #0ff
                    text_style +: {
                        font_size: 20
                    }
                }
                text: "LabelGradientY"
            }

            Hr{}
            H4{text: "TextBox"}
            TextBox{
                text: "Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo. Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit, sed quia consequuntur magni dolores eos qui ratione voluptatem sequi nesciunt."
            }

            Hr{}
            H4{text: "Typographic System"}
            H1{text: "H1 headline"}
            H1italic{text: "H1 italic headline"}
            H2{text: "H2 headline"}
            H2italic{text: "H2 italic headline"}
            H3{text: "H3 headline"}
            H3italic{text: "H3 italic headline"}
            H4{text: "H4 headline"}
            H4italic{text: "H4 italic headline"}
            P{text: "P copy text"}
            Pitalic{text: "P italic copy text"}
            Pbold{text: "P bold copy text"}
            Pbolditalic{text: "P bold italic copy text"}

            Hr{}
            H4{text: "Styling Attributes Reference"}
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

            Hr{}
            H4{text: "Ellipsis (single line)"}
            P{text: "Long ASCII text truncated to 1 line with ellipsis. Resize the window to see the truncation point move."}
            Label{
                width: Fill
                max_lines: 1
                text_overflow: Ellipsis
                text: "This is a very long label text that should be truncated with an ellipsis character at the end of the line when it overflows"
            }

            Hr{}
            H4{text: "Ellipsis (2 lines max)"}
            P{text: "Text wraps up to 2 lines, then truncates with ellipsis. Resize to see lines reflow."}
            Label{
                width: Fill
                max_lines: 2
                text_overflow: Ellipsis
                text: "This is a longer piece of text that should wrap to multiple lines. When it exceeds two lines, it should be truncated with an ellipsis character appended to the end of the second line. Try resizing the window to watch the wrapping and truncation adapt dynamically."
            }

            Hr{}
            H4{text: "Ellipsis (3 lines max, Fill width)"}
            P{text: "A TextBox (Fill width) with max_lines: 3 and text_overflow: Ellipsis."}
            TextBox{
                max_lines: 3
                text_overflow: Ellipsis
                text: "Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo. Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit, sed quia consequuntur magni dolores eos qui ratione voluptatem sequi nesciunt."
            }

            Hr{}
            H4{text: "Ellipsis with emoji"}
            P{text: "Emoji are multi-byte (up to 4 bytes). Resize to see truncation at emoji boundaries."}
            Label{
                width: Fill
                max_lines: 1
                text_overflow: Ellipsis
                text: "Stars \u{2B50}\u{2B50}\u{2B50} and rockets \u{1F680}\u{1F680}\u{1F680} and flags \u{1F3C1}\u{1F3C1}\u{1F3C1} and more emoji to overflow the line boundary"
            }

            Hr{}
            H4{text: "Ellipsis with CJK characters"}
            P{text: "CJK characters are 3 bytes each in UTF-8 and wider than Latin glyphs."}
            Label{
                width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.6}}
                max_lines: 1
                text_overflow: Ellipsis
                text: "\u{6587}\u{5B57}\u{306E}\u{30C6}\u{30B9}\u{30C8}\u{3067}\u{3059}\u{3002}\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{6587}\u{7AE0}\u{304C}\u{9577}\u{3059}\u{304E}\u{308B}\u{3068}\u{7701}\u{7565}\u{8A18}\u{53F7}\u{304C}\u{8868}\u{793A}\u{3055}\u{308C}\u{307E}\u{3059}"
            }

            Hr{}
            H4{text: "Ellipsis with mixed scripts (2 lines)"}
            P{text: "Latin, Cyrillic, and emoji mixed together with 2-line wrapping."}
            Label{
                width: Fill
                max_lines: 2
                text_overflow: Ellipsis
                text: "Hello \u{041F}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{1F44B} world! Multi-script text with various byte lengths per character should wrap and truncate cleanly across line boundaries when the window is resized."
            }

            Hr{}
            H4{text: "Ellipsis with dense emoji (single line)"}
            P{text: "A string of only emoji — every character is 4 bytes."}
            Label{
                width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.5}}
                max_lines: 1
                text_overflow: Ellipsis
                text: "\u{1F600}\u{1F601}\u{1F602}\u{1F603}\u{1F604}\u{1F605}\u{1F606}\u{1F607}\u{1F608}\u{1F609}\u{1F60A}\u{1F60B}\u{1F60C}\u{1F60D}\u{1F60E}\u{1F60F}\u{1F610}\u{1F611}\u{1F612}\u{1F613}"
            }

            Hr{}
            H4{text: "No truncation (short text)"}
            P{text: "Text fits within max_lines — no ellipsis shown."}
            Label{
                width: Fill
                max_lines: 1
                text_overflow: Ellipsis
                text: "Short text"
            }

            Hr{}
            H4{text: "Ellipsis (Fill width)"}
            P{text: "A label that fills the parent width and truncates with ellipsis."}
            Label{
                width: Fill
                max_lines: 1
                text_overflow: Ellipsis
                text: "This label fills the entire available width. When the text is too long to fit on a single line, it is truncated and an ellipsis is appended at the end to indicate there is more content."
            }

            Hr{}
            H4{text: "Ellipsis (Fit width with max bound)"}
            P{text: "Fit width capped at 50% of the parent. Text grows until hitting the max, then truncates."}
            Label{
                width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.5}}
                max_lines: 1
                text_overflow: Ellipsis
                text: "This label uses Fit width with a relative max bound of 50%. Short text grows naturally, but long text like this gets truncated with an ellipsis once it hits the maximum width."
            }

            Hr{}
            H4{text: "Custom Shader"}
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
        }
    }
}
