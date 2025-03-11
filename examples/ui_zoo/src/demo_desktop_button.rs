use crate::{
    makepad_widgets::*,
    makepad_widgets::file_tree::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;

    pub DemoDesktopButtonDemo = {{DemoDesktopButton}} {
        <UIZooTab> {
            align: { x: 0.5, y: 0.5 }
            flow: Right,
            desc = {
                <Label> { text: "test"}
            }
            demos = {
                <DesktopButton> { draw_bg: { button_type: WindowsMax} }
                <DesktopButton> { draw_bg: { button_type: WindowsMax} }
                <DesktopButton> { draw_bg: { button_type: WindowsMax} }
                <DesktopButton> { draw_bg: { button_type: WindowsMax} }
                <DesktopButton> { draw_bg: { button_type: WindowsMaxToggled} }
                <DesktopButton> { draw_bg: { button_type: WindowsClose} }
                <DesktopButton> { draw_bg: { button_type: XRMode} }
                <DesktopButton> { draw_bg: { button_type: Fullscreen } }
            }
        }
    }

}