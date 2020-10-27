// a window menu implementation
use makepad_render::*;

#[derive(Clone)]
pub struct WindowMenu {
    pub view: View,
    pub item_draw: MenuItemDraw,
}

#[derive(Clone)]
pub struct MenuItemDraw {
    pub text: Text,
    pub item_bg: Quad,
}

impl MenuItemDraw {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            text: Text {
                wrapping: Wrapping::Word,
                ..Text::new(cx)
            },
            item_bg: Quad::new(cx),
        }
    }
}

#[derive(Clone)]
pub enum WindowMenuEvent {
    SelectItem {
    },
    None,
}

impl WindowMenu {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            item_draw: MenuItemDraw::new(cx),
            view: View::new(cx),
        }
    }
    
    pub fn handle_window_menu(&mut self, _cx: &mut Cx, _event: &mut Event, _menu: &Menu) -> WindowMenuEvent {
        WindowMenuEvent::None
    }
    
    pub fn draw_window_menu(&mut self, _cx: &mut Cx, _menu: &Menu) {
    }
}
