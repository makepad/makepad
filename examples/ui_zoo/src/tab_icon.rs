use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoIcon = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/icon.md") } 
        }
        demos = {
            <H4> { text: "Standard" }
            <Icon> {
                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
            }

            <Hr> {}
            <H4> { text: "IconGradientX" }
            <IconGradientX> {
                icon_walk: { width: 100.  }
                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
            }
            
            <Hr> {}
            <H4> { text: "IconGradientY" }
            <IconGradientY> {
                icon_walk: { width: 100.  }
                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
            }

            <H4> { text: "Styling Attributes Reference" }
            <Icon> {
                width: Fit,
                height: Fit,

                icon_walk: {
                    width: 50.
                    margin: 10.
                }

                draw_bg: { color: #0 }
                draw_icon: { color: #f0f }
                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg") }
            }
        }
    }
}