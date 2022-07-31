use {
    crate::{
        makepad_platform::*,
        imgui::*,
        frame::*,
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
    fn button(&mut self, label: &str) -> ButtonImGUI;
}

impl<'a> ButtonImGUIExt for ImGUIRun<'a> {
    fn button(&mut self, text: &str) -> ButtonImGUI {
        let new_id = id_num!(button, self.alloc_auto_id());
        let mut frame = self.imgui.frame();
        let button = if let Some(button) = frame.component_by_path(&[new_id]) {
            Some(button)
        }
        else {
            frame.template(self.cx, ids!(button), new_id, live!{
                text: (text)
            })
        };
        ButtonImGUI(self.safe_ref::<Button>(button))
    }
}
