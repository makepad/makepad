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
            <H3> { text: "<DesktopButton>"}
        }
        demos = {
            <DesktopButton> { draw_bg: { button_type: WindowsMax} }
            <DesktopButton> { draw_bg: { button_type: WindowsMaxToggled} }
            <DesktopButton> { draw_bg: { button_type: WindowsClose} }
            <DesktopButton> { draw_bg: { button_type: XRMode} }
            <DesktopButton> { draw_bg: { button_type: Fullscreen } }
        }
    }
}