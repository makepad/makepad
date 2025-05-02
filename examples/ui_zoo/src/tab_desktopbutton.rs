use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoDesktopButton = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/desktopbutton.md") } 
        }
        demos = {
            <H4> { text: "WindowsMax" }
            <DesktopButton> { draw_bg: { button_type: WindowsMax} }

            <Hr> {}
            <H4> { text: "WindowsMaxToggled" }
            <DesktopButton> { draw_bg: { button_type: WindowsMaxToggled} }

            <Hr> {}
            <H4> { text: "WindowsClose" }
            <DesktopButton> { draw_bg: { button_type: WindowsClose} }

            <Hr> {}
            <H4> { text: "XRMode" }
            <DesktopButton> { draw_bg: { button_type: XRMode} }

            <Hr> {}
            <H4> { text: "Fullscreen" }
            <DesktopButton> { draw_bg: { button_type: Fullscreen } }
        }
    }
}