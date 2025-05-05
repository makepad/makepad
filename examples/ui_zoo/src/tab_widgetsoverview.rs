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
            flow: Down,
            align: {x: 0.5, y: 0.5}
            height: Fill, width: Fill,

            <ScrollYView> {
                flow: Down
                width: 430., height: Fill,
                align: {x: 0.0, y: 0.4}
                spacing: (THEME_SPACE_3)

                <Image> { margin: {bottom: 10.}, width: 250, height: 36.5, source: dep("crate://self/resources/logo_makepad.png" ), fit: Biggest }

                <H4> { text: "Makepad is an open-source, cross-platform UI framework written in and for Rust. It runs natively and on the web, supporting all major platforms: Windows, Linux, macOS, iOS, and Android." } 
                <P> {
                    text: "Built on a shader-based architecture, Makepad delivers high performance, making it suitable for complex applications like Photoshop or even 3D/VR/AR experiences."
                }
                <P> {
                    text: "It compiles exceptionally fast, ensuring a smooth and interruption-free development cycle."
                }
                <P> {
                    text: "One of Makepad’s standout features is live styling — a powerful system that reflects UI code changes instantly without recompilation or restarts. This tight feedback loop bridges the gap between developers and designers, streamlining collaboration and maximizing productivity."
                }
                <P> {
                    text: "This example application provides an overview of the currently supported widgets and their variants."
                }
                <P> {
                    text: "Its source code also provides useful example code to get you started."
                }

                <TextBox> { draw_text: { color: (THEME_COLOR_MAKEPAD) }, height: Fit, text: "\nUI Zoo hosts an unusually large number of widgets, resulting in loading times that aren’t representative of typical Makepad applications." }
            }


            // Overview for making sure that all controls have consistent heights and line up cleanly. Determining factor for cleanly lining up: the label baselines are all aligned.
            // <View> {
            //     <P> { width: Fit, text: "P-text"}
            //     <LinkLabel> {text: "LinkLabel"}
            //     <Vr> {}
            //     <Label> {text: "Label"}
            //     <Button> { text: "Button"}
            //     dropdown_demo = <DropDown> {
            //         popup_menu_position: BelowInput,
            //         labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
            //         values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
            //     }
            //     <TextInput> { empty_text: "TextInput", width: Fill }
            //     <SliderRound> { text: "SliderRound"}
            //     <Toggle> { text: "Toggle"}
            //     <CheckBox> { text: "CheckBox"}
            //     <CheckBoxCustom> {
            //         draw_bg: { check_type: None }
            //         text:"Custom Checkbox"
            //         draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
            //         label_walk: {
            //             width: Fit, height: Fit,
            //             margin: <THEME_MSPACE_H_1> { left: 5. }
            //         }

            //         draw_text: {
            //             text_style: <THEME_FONT_REGULAR> {
            //                 font_size: (THEME_FONT_SIZE_P)
            //             }
            //         }

            //         draw_icon: { color_active: #f00 }
            //     }
            //     <CheckBox> { text: "CheckBox"}
            //     radios_demo_20 = <View> {
            //         spacing: (THEME_SPACE_2)
            //         width: Fit, height: Fit,
            //         radio1 = <RadioButton> { text: "Option 1" }
            //         radio2 = <RadioButton> { text: "Option 2" }
            //         radio3 = <RadioButton> { text: "Option 3" }
            //         radio4 = <RadioButton> { text: "Option 4" }
            //     }
            //     <Slider> { text: "Slider"}
            //     <SliderMinimal> { text: "Slider"}
            //     <Rotary> { text: "Rotary" }
            // }

    }
}