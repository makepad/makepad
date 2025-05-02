use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub WidgetsOverview = <View> {
            spacing: (THEME_SPACE_2)
            padding: <THEME_MSPACE_2> {}
            flow: Right,
            height: Fill,
            width: Fill,

            <ScrollYView> {
                flow: Down
                width: Fill, height: Fill,
                spacing: (THEME_SPACE_3)

                <View> {
                    height: Fit, width: Fill,
                    <P> { text: "Label", width: Fit}
                    <LinkLabel> { text: "Link", width: Fit}
                    <FoldButton> { }
                }
                <View> {
                    height: Fit, width: Fill,
                    <CheckBox> { text: "CheckBox"}
                    <CheckBox> { text: "CheckBox"}
                    <CheckBox> { text: "CheckBox"}
                }
                <View> {
                    height: Fit, width: Fill,
                    <CheckBoxCustom> {
                        draw_bg: { check_type: None }
                        padding: <THEME_MSPACE_V_1> {}
                        text:"Custom Checkbox"
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                        label_walk: {
                            width: Fit, height: Fit,
                            margin: <THEME_MSPACE_H_1> { left: 5. }
                        }

                        draw_text: {
                            text_style: <THEME_FONT_REGULAR> {
                                font_size: (THEME_FONT_SIZE_P)
                            }
                        }

                        draw_icon: { color_active: #f00 }

                        icon_walk: { width: 13.0, height: Fit }
                    }

                }
                <View> {
                    height: Fit, width: Fill,
                    <Toggle> { text: "Toggle"}
                    <Toggle> { text: "Toggle"}
                    <Toggle> { text: "Toggle"}
                }
                <Button> { text: "Button", width: Fill}
                <TextInput> { empty_text: "TextInput", width: Fill }
                <SliderMinimal> { text: "SliderMinimal"}
                <Slider> { text: "Slider"}
                <SliderRound> { text: "SliderRound"}
                <View> {
                    height: Fit,
                    align: { x: 0.5 }
                    spacing: (THEME_SPACE_3)

                    <Rotary> { text: "Rotary" }
                    <Rotary> { text: "Rotary" }
                    <Rotary> { text: "Rotary" }
                }
                dropdown_demo = <DropDown> {
                    width: Fill,
                    labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                }
                radios_demo_20 = <View> {
                    spacing: (THEME_SPACE_2)
                    width: Fit, height: Fit,
                    radio1 = <RadioButton> { text: "Option 1" }
                    radio2 = <RadioButton> { text: "Option 2" }
                    radio3 = <RadioButton> { text: "Option 3" }
                    radio4 = <RadioButton> { text: "Option 4" }
                }
            }
            <ScrollYView> {
                flow: Down
                width: Fill, height: Fill,
                spacing: (THEME_SPACE_3)

                <Image> { width: Fill, height: Fit, source: dep("crate://self/resources/ducky.png" ), fit: Biggest }
                <Icon> {
                    icon_walk: { width: 100.  }
                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                }
                <IconSet> {
                    text: ""
                    draw_text: { color: #fff }
                }
            }
            <View> {
                flow: Down
                width: Fill, height: Fill,
                spacing: (THEME_SPACE_2)
            }
    }
}