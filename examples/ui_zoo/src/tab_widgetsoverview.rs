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
            <Markdown> { body: dep("crate://self/resources/widgetsoverview.md") } 
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
                <Toggle> { text: "TestButton"}
                <ButtonFlat> { text: "TestButton"}
                <Button> { text: "TestButton, disabled", enabled: true}
                <TextInput> { text: "TestButton"}
                <Slider> { text: "TestButton"}
                <Slider> { text: "TestButton"}
            }

        }
    }
}