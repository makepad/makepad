use crate::cx::*;
use makepad_platform::{
    cursor::MouseCursor
};

impl Cx {
    pub fn set_down_mouse_cursor(&mut self, mouse_cursor: MouseCursor) {
        // ok so lets set the down mouse cursor
        self.down_mouse_cursor = Some(mouse_cursor);
    }
    pub fn set_hover_mouse_cursor(&mut self, mouse_cursor: MouseCursor) {
        // the down mouse cursor gets removed when there are no captured fingers
        self.hover_mouse_cursor = Some(mouse_cursor);
    }
}