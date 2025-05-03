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
            <Markdown> { body: dep("crate://self/resources/dropdown.md") } 
        }
        demos = {
            <H4> { text: "Standard" }
            dropdown = <DropDown> {
            labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <H4> { text: "Standard, Position: BelowInput" }
            dropdown_below = <DropDown> {
            popup_menu_position: BelowInput,
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "Standard, disabled" }
            dropdown_disabled = <DropDown> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                animator: {
                    disabled = {
                        default: on
                    }
                }
            }

            <Hr> {}
            <H4> { text: "DropDownFlat" }
            dropdown_flat = <DropDownFlat> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <H4> { text: "DropDownFlat, Position: BelowInput" }
            dropdown_flat_below = <DropDownFlat> {
                popup_menu_position: BelowInput,
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "DropDownFlatter" }
            dropdown_flatter = <DropDownFlatter> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <H4> { text: "DropDownFlatter, Position: BelowInput" }
            dropdown_flatter_below = <DropDownFlatter> {
                popup_menu_position: BelowInput,
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "DropDownGradientX" }
            dropdown_gradient_x = <DropDownGradientX> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <H4> { text: "DropDownGradientX, Position: BelowInput" }
            dropdown_gradient_x_below = <DropDownGradientX> {
                popup_menu_position: BelowInput,
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <Hr> {}
            <H4> { text: "DropDownGradientY" }
            dropdown_gradient_y = <DropDownGradientY> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            }

            <H4> { text: "DropDownGradientY, Position: BelowInput" }
            dropdown_gradient_y_below = <DropDownGradientY> {
                popup_menu_position: BelowInput,
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

                        color_1: #4
                        color_2: #2

                        border_color_1: #C
                        border_color_2: #C
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Standard, fully customized" }
            dropdown_customized = <DropDown> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]

                popup_menu_position: BelowInput,

                width: Fill, height: Fit,
                align: { x: 0., y: .5 }
                padding: 10.
                margin: 10.
            
                draw_text: {
                    color: #4
                    color_hover: #6
                    color_down: #0
                    color_focus: #8
                    color_disabled: #C

                    text_style: {
                        font_size: 8.,
                        line_spacing: 1.4,
                        font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                    }
                }

                draw_bg: {
                    border_size: (THEME_BEVELING)
                    border_radius: (THEME_CORNER_RADIUS)

                    color_dither: 1.0

                    color: #A
                    color_hover: #C
                    color_down: #9
                    color_focus: #B
                    color_disabled: #8

                    border_color_1: #0
                    border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_1_HOVER)
                    border_color_1_down: (THEME_COLOR_BEVEL_OUTSET_1_DOWN)
                    border_color_1_focus: (THEME_COLOR_BEVEL_OUTSET_1_FOCUS)
                    border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_1_DISABLED)

                    border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
                    border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2_HOVER)
                    border_color_2_down: (THEME_COLOR_BEVEL_OUTSET_2_DOWN)
                    border_color_2_focus: (THEME_COLOR_BEVEL_OUTSET_2_FOCUS)
                    border_color_2_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)

                    arrow_color: (THEME_COLOR_LABEL_INNER)
                    arrow_color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
                    arrow_color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
                    arrow_color_down: (THEME_COLOR_LABEL_INNER_DOWN)
                    arrow_color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)
                }


            }


        }
    }
}