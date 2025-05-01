use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoLabel = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<Label>"}
        }
        demos = {
            <H4> { text: "Standard" }
            <Label> { text:"Default single line text" }
            
            <Hr> {}
            <H4> { text: "Styled" }
            <Label> {
                draw_text: {
                    color: (THEME_COLOR_MAKEPAD)
                    text_style: {
                        font_size: 20,
                    }
                },
                text: "You can style text using colors and fonts"
            }
            
            <Hr> {}
            <H4> { text: "LabelGradientX" }
            <LabelGradientX> { text: "<LabelGradientY>" }
            <LabelGradientX> {
                draw_text: {
                    color_1: #0ff
                    color_1: #088
                    text_style: {
                        font_size: 20,
                    }
                },
                
                text: "<LabelGradientX>"
            }
            
            <Hr> {}
            <H4> { text: "LabelGradientY" }
            <LabelGradientY> { text: "<LabelGradientY>" }
            <LabelGradientY> {
                draw_text: {
                    color_1: #0ff
                    color_1: #088
                    text_style: {
                        font_size: 20,
                    }
                },
                
                text: "<LabelGradientY>"
            }
            
            <Hr> {}
            <H4> { text: "Customized" }
            <Label> {
                draw_text: {
                    fn get_color(self) ->vec4{
                        return mix((THEME_COLOR_MAKEPAD), (THEME_COLOR_U_HIDDEN), self.pos.x)
                    }
                    color: (THEME_COLOR_MAKEPAD)
                    text_style: {
                        font_size: 40.,
                    }
                },
                text: "OR EVEN SOME PIXELSHADERS"
            }

            <Hr> {}
            <H4> { text: "TextBox" }
            <TextBox> {
                text: "Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo. Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit, sed quia consequuntur magni dolores eos qui ratione voluptatem sequi nesciunt. Neque porro quisquam est, qui dolorem ipsum quia dolor sit amet, consectetur, adipisci velit, sed quia non numquam eius modi tempora incidunt ut labore et dolore magnam aliquam quaerat voluptatem. Ut enim ad minima veniam, quis nostrum exercitationem ullam corporis suscipit laboriosam, nisi ut aliquid ex ea commodi consequatur? Quis autem vel eum iure reprehenderit qui in ea voluptate velit esse quam nihil molestiae consequatur, vel illum qui dolorem eum fugiat quo voluptas nulla pariatur?"
            }

            
            <Hr> {}
            <H4> { text: "Typographic System" }
            <H1> { text: "H1 headline" }
            <H1italic> { text: "H1 italic headline" }
            <H2> { text: "H2 headline" }
            <H2italic> { text: "H2 italic headline" }
            <H3> { text: "H3 headline" }
            <H3italic> { text: "H3 italic headline" }
            <H4> { text: "H4 headline" }
            <H4italic> { text: "H4 italic headline" }
            <P> { text: "P copy text" }
            <Pitalic> { text: "P italic copy text" }
            <Pbold> { text: "P bold copy text" }
            <Pbolditalic> { text: "P bold italic copy text" }
        }
    }
}