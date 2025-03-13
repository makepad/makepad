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
            <H3> { text: "<Icon>"}
        }
        demos = {
            <Icon> {
                icon_walk: { width: 100.  }
                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
            }

            <IconGradientX> {
                icon_walk: { width: 100.  }
                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
            }
            
            <IconGradientY> {
                icon_walk: { width: 100.  }
                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
            }
        }
    }
}