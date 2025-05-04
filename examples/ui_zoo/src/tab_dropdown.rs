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
            <H4> { text: "Styling Attributes Reference" }
            dropdown_customized = <DropDown> {
                labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                popup_menu: <PopupMenu> {}
                selected_item: 0
                popup_menu_position: BelowInput,

                width: Fit, height: Fit,
                align: {x: 0., y: 0.}

                padding: <THEME_MSPACE_1> { left: (THEME_SPACE_2), right: 22.5 }
                margin: <THEME_MSPACE_V_1> {}
            
                draw_text: {
                    color: (THEME_COLOR_LABEL_INNER)
                    color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
                    color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
                    color_down: (THEME_COLOR_LABEL_INNER_DOWN)
                    color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

                    text_style: {
                        font_size: (THEME_FONT_SIZE_P)
                        font_family: {
                            latin = font("crate://makepad_widgets/resources/IBMPlexSans-Text.ttf", -0.1, 0.0),
                            chinese = font("crate://makepad_widgets/resources/LXGWWenKaiRegular.ttf", 0.0, 0.0)
                            emoji = font("crate://makepad_widgets/resources/NotoColorEmoji.ttf", 0.0, 0.0)
                        },
                        line_spacing: 1.2
                    }
                }

                draw_bg: {
                    border_size: (THEME_BEVELING)
                    border_radius: (THEME_CORNER_RADIUS)

                    color_dither: 1.0

                    color: (THEME_COLOR_OUTSET)
                    color_hover: (THEME_COLOR_OUTSET_HOVER)
                    color_down: (THEME_COLOR_OUTSET_DOWN)
                    color_focus: (THEME_COLOR_OUTSET_FOCUS)
                    color_disabled: (THEME_COLOR_OUTSET_DISABLED)

                    border_color_1: (THEME_COLOR_BEVEL_OUTSET_1)
                    border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_1_HOVER)
                    border_color_1_focus: (THEME_COLOR_BEVEL_OUTSET_1_FOCUS)
                    border_color_1_down: (THEME_COLOR_BEVEL_OUTSET_1_DOWN)
                    border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_1_DISABLED)

                    border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
                    border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2_HOVER)
                    border_color_2_focus: (THEME_COLOR_BEVEL_OUTSET_2_FOCUS)
                    border_color_2_down: (THEME_COLOR_BEVEL_OUTSET_2_DOWN)
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