// a window menu implementation
use crate::makepad_platform::*;

live_register!{
}

#[derive(Clone)]
pub struct WindowMenu {
}

#[derive(Clone)]
pub enum WindowMenuEvent {
    SelectItem {
    },
    None,
}

impl WindowMenu {
    pub fn new(_cx: &mut Cx) -> Self {
        Self {
        }
    }
    
    pub fn handle_window_menu(&mut self, _cx: &mut Cx, _event: &mut Event, _menu: &Menu) -> WindowMenuEvent {
        WindowMenuEvent::None
    }
    
    pub fn draw_window_menu(&mut self, _cx: &mut Cx, _menu: &Menu) {
    }
}
