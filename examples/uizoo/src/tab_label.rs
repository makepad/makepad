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
