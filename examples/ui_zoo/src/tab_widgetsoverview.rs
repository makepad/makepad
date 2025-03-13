use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub WidgetsOverview = <UIZooTabLayout_B> {
        desc = {
        }
        demos = {
            spacing: (THEME_SPACE_2)
            padding: <THEME_MSPACE_2> {}
            <View> {
                padding: <THEME_MSPACE_2> {}
                spacing: (THEME_SPACE_2)
                flow: Right,
                height: Fit,

                <P> { text: "TestLabel", width: Fit}
                <LinkLabel> { text: "TestButton", width: Fit}
                <FoldButton> {
                    height: 25, width: 15,
                    margin: { left: (THEME_SPACE_2) }
                    animator: { open = { default: off } },
                }

                <CheckBox> { text: "TestButton"}
                <CheckBoxToggle> { text: "TestButton"}
                <ButtonFlat> { text: "TestButton"}
                <Button> { text: "TestButton, disabled", enabled: true}
                <TextInput> { text: "TestButton"}
                <Slider> { text: "TestButton"}
                <SliderBig> { text: "TestButton"}
            }

            <Hr> {}

            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                <Button> { text: "<Button>" }
                <ButtonIcon> {
                    draw_icon: {
                        color: #f00,
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<ButtonIcon>"
                }
                <ButtonFlat> { text: "<ButtonFlat>"}
                <ButtonFlatter> { text: "<ButtonFlatter>"}
                <ButtonGradientY> { text: "<ButtonGradientX>" }
            }
            <Hr> {}
            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                <LinkLabel> { text: "<LinkLabel>", width: Fit}
                <LinkLabelGradientX> { text: "<LinkLabelGradientX>", width: Fit}
                <LinkLabelGradientY> { text: "<LinkLabelGradientY>", width: Fit}
            }
            <Hr> {}
            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                <Icon> {
                    icon_walk: { width: 50.  }
                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                }

                <IconGradientX> {
                    icon_walk: { width: 50.  }
                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                }
                
                <IconGradientY> {
                    icon_walk: { width: 50.  }
                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                }
            }
            <Hr> {}
            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                <CheckBox> { text: "<CheckBox>"}
                <CheckBoxToggle> { text: "<CheckBoxToggle>"}
                <CheckBoxCustom> {
                    text:"<CheckBoxCustom>"
                    draw_bg: { check_type: None }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                }
                <CheckBoxCustom> {
                    text:"<CheckBoxCustom>"
                    draw_bg: { check_type: None }
                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg") }
                    draw_icon: {
                        color: #300
                        color_hover: #800
                        color_active: #F00
                    }
                    icon_walk: { width: 13.0, height: Fit }
                }
            }
            <Hr> {}
            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                <DesktopButton> { draw_bg: { button_type: WindowsMax} }
                <DesktopButton> { draw_bg: { button_type: WindowsMaxToggled} }
                <DesktopButton> { draw_bg: { button_type: WindowsClose} }
                <DesktopButton> { draw_bg: { button_type: XRMode} }
                <DesktopButton> { draw_bg: { button_type: Fullscreen } }
            }
            <Hr> {}
            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                dropdown = <DropDown> {
                    labels: ["<DropDown>", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                }
                dropdown_flat = <DropDownFlat> {
                    labels: ["<DropDownFlat>", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                }
                dropdown_gradient_x = <DropDownGradientX> {
                    labels: ["<DropDownGradientX>", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                }
                dropdown_gradient_y = <DropDownGradientY> {
                    labels: ["<DropDownGradientY>", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                }
            }
            <Hr> {}
            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                <TextInput> { empty_message: "<TextInput>"}
                <TextInputGradientY> { empty_message: "<TextInputGradientY>"}
            }
            <Hr> {}
            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                width: Fit,
                <View> {
                    flow: Down,
                    radio1 = <RadioButton> { text: "<RadioButton>" }
                    radio2 = <RadioButton> { text: "Option 2" }
                    radio3 = <RadioButton> { text: "Option 3" }
                    radio4 = <RadioButton> { text: "Option 4" }
                }

                iconradios_demo = <View> {
                    width: Fit, height: Fit,
                    spacing: (THEME_SPACE_2)
                    flow: Down,

                    radio1 = <RadioButtonCustom> {
                        text: "<RadioButtonCustom> 1"
                        icon_walk: { width: 12.5, height: Fit }
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                        
                    }
                    radio2 = <RadioButtonCustom> {
                        text: "<RadioButtonCustom> 2"
                        icon_walk: { width: 12.5, height: Fit, }
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                    }
                    radio3 = <RadioButtonCustom> {
                        text: "<RadioButtonCustom> 3"
                        icon_walk: { width: 12.5, height: Fit, }
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                    }
                    radio4 = <RadioButtonCustom> {
                        text: "<RadioButtonCustom> 4"
                        icon_walk: { width: 12.5, height: Fit, }
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                    }
                }

                textonlyradios_demo = <View> {
                    width: Fit, height: Fit,
                    flow: Down,
                    spacing: (THEME_SPACE_2)
                    radio1 = <RadioButtonTextual> { text: "<RadioButtonTextual> 1" }
                    radio2 = <RadioButtonTextual> { text: "<RadioButtonTextual> 2" }
                    radio3 = <RadioButtonTextual> { text: "<RadioButtonTextual> 3" }
                    radio4 = <RadioButtonTextual> { text: "<RadioButtonTextual> 4" }
                }

                radiotabs_demo = <View> {
                    spacing: 5.
                    width: Fit, height: Fit,
                    flow: Down,
                    radio1 = <RadioButtonTab> { text: "<RadioButtonTab> 1" }
                    radio2 = <RadioButtonTab> { text: "<RadioButtonTab> 2" }
                    radio3 = <RadioButtonTab> { text: "<RadioButtonTab> 3" }
                    radio4 = <RadioButtonTab> { text: "<RadioButtonTab> 4" }
                }

                <ButtonGroup> {
                    height: Fit
                    align: { x: 0.0, y: 0.5 }
                    radiotabs_demo = <View> {
                        flow: Down,
                        spacing: 5.
                        width: Fit, height: Fit,
                        radio1 = <RadioButtonTabGradientY> { text: "<RadioButtonTabGradientY> 1" }
                        radio2 = <RadioButtonTabGradientY> { text: "<RadioButtonTabGradientY> 2" }
                        radio3 = <RadioButtonTabGradientY> { text: "<RadioButtonTabGradientY> 3" }
                        radio4 = <RadioButtonTabGradientY> { text: "<RadioButtonTabGradientY> 4" }
                    }
                }
            }
            <Hr> {}
            <View> {
                flow: Right,
                spacing: (THEME_SPACE_2)
                height: Fit,
                <Slider> { text: "TestButton"}
                <SliderBig> { text: "TestButton"}
                <SliderAlt1> { text: "TestButton"}
                <Rotary> { text: "TestButton"}
                <RotaryFlat> { text: "TestButton"}
                <RotarySolid> { text: "TestButton"}
            }

        }
    }
}