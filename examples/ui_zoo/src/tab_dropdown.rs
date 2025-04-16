use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoDropdown = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<DropDown>"}
        }
        demos = {
            <H4> { text: "Standard" }
            dropdown = <DropDown> {
            labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <H4> { text: "Standard, Position: BelowInput" }
            dropdown_below = <DropDown> {
            popup_menu_position: BelowInput ,
            labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "DropDownFlat" }
            dropdown_flat = <DropDownFlat> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "DropDownFlatter" }
            dropdown_flatter = <DropDownFlatter> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "DropDownGradientX" }
            dropdown_gradient_x = <DropDownGradientX> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "DropDownGradientY" }
            dropdown_gradient_y = <DropDownGradientY> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "DropDownGradientY Custom" }
            dropdown_custom = <DropDownGradientY> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]

                
                draw_text: {
                    color: #f00
                    color_hover: #0f0
                    color_focus: #0ff
                }

            
                draw_bg: {
                    border_size: (THEME_BEVELING)
                    border_radius: (THEME_CORNER_RADIUS)

                    color_dither: 1.0

                    color_1: (THEME_COLOR_OUTSET * 1.75)
                    color_1_hover: #0
                    color_1_focus: #2

                    color_2: (THEME_COLOR_OUTSET)
                    color_2_hover: #3
                    color_2_focus: #4

                    border_color_1: #8
                    border_color_1_hover: #C
                    border_color_1_focus: #A

                    border_color_2: #8
                    border_color_2_hover: #C
                    border_color_2_focus: #A
                }

                popup_menu: <PopupMenuGradientY> {
                    // menu_item: <PopupMenuItem> {}

                    draw_bg: {
                        color_dither: 1.0
                        border_radius: 4.0
                        border_size: (THEME_BEVELING)
                        inset: vec4(0.0, 0.0, 0.0, 0.0),

                        color_1: #4
                        color_2: #2

                        border_color_1: #C
                        border_color_2: #C
                    }
                }
            }
        }
    }
}