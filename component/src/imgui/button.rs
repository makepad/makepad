use {
    crate::{
        makepad_platform::*,
        imgui::*,
        button::*,
    }
};


// ImGUI API for Button

pub struct ButtonImGUI(ImGUIRef); 

impl ButtonImGUI {
    pub fn was_clicked(&self) -> bool {
        if let Some(item) = self.0.find_single_action() {
            if let ButtonAction::WasClicked = item.action() {
                return true
            }
        }
        false
    }
    pub fn inner(&mut self) -> Option<std::cell::RefMut<'_, Button >> {
        self.0.inner()
    }
}

pub trait ButtonImGUIExt {
    fn button(&mut self, path: &[LiveId]) -> ButtonImGUI;
}

impl<'a> ButtonImGUIExt for ImGUIRun<'a> {
    fn button(&mut self, path: &[LiveId]) -> ButtonImGUI {
        let mut frame = self.imgui.frame();
        ButtonImGUI(self.safe_ref::<Button>(frame.component_by_path(path)))
    }
}
