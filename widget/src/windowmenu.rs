// a window menu implementation
use makepad_render::*;

#[derive(Clone)]
pub struct WindowMenu {
    pub view: View,
    pub item_draw: MenuItemDraw,
}

#[derive(Clone)]
pub struct MenuItemDraw {
    pub text: DrawText,
    pub item_bg: DrawQuad,
}

impl MenuItemDraw {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            text: DrawText::new(cx, default_shader!())
                .with_wrapping(Wrapping::Word),
            item_bg: DrawQuad::new(cx, default_shader!()),
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
            view: View::new(),
        }
    }
    
    pub fn handle_window_menu(&mut self, _cx: &mut Cx, _event: &mut Event, _menu: &Menu) -> WindowMenuEvent {
        WindowMenuEvent::None
    }
    
    pub fn draw_window_menu(&mut self, _cx: &mut Cx, _menu: &Menu) {
    }
}
