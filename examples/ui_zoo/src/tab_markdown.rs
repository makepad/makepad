use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoMarkdown = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/markdown.md") } 
        }
        demos = {
            <Markdown> {
                width:Fill, height: Fit,
                body:"# Headline 1 \n ## Headline 2 \n ### Headline 3 \n #### Headline 4 \n This is standard text with a  \n\n line break a short ~~strike through~~ demo.\n\n *Italic text* \n\n **Bold text** \n\n - Bullet\n - Another bullet\n - Third bullet\n 1. Numbered list Bullet\n 2. Another list entry\n 3. Third list entry\n `Monospaced text`\n> This is a quote.\nThis is `inline code`.\n ```code block```"
            }
        }
    }
}