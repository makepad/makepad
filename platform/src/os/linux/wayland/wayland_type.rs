use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1;
use crate::{MouseButton, MouseCursor};

impl Into<wp_cursor_shape_device_v1::Shape> for MouseCursor {
    fn into(self) -> wp_cursor_shape_device_v1::Shape {
        match self {
            // hidden need to be processed by WlPointer
            MouseCursor::Hidden => wp_cursor_shape_device_v1::Shape::Default,
            MouseCursor::Default => wp_cursor_shape_device_v1::Shape::Default,
            MouseCursor::Crosshair => wp_cursor_shape_device_v1::Shape::Crosshair,
            MouseCursor::Hand => wp_cursor_shape_device_v1::Shape::Pointer,
            MouseCursor::Arrow => wp_cursor_shape_device_v1::Shape::ContextMenu,
            MouseCursor::Move => wp_cursor_shape_device_v1::Shape::Move,
            MouseCursor::Text => wp_cursor_shape_device_v1::Shape::Text,
            MouseCursor::Wait => wp_cursor_shape_device_v1::Shape::Wait,
            MouseCursor::Help => wp_cursor_shape_device_v1::Shape::Help,
            MouseCursor::NotAllowed => wp_cursor_shape_device_v1::Shape::NotAllowed,
            MouseCursor::Grab => wp_cursor_shape_device_v1::Shape::Grab,
            MouseCursor::Grabbing => wp_cursor_shape_device_v1::Shape::Grabbing,
            MouseCursor::NResize => wp_cursor_shape_device_v1::Shape::NResize,
            MouseCursor::NeResize => wp_cursor_shape_device_v1::Shape::NeResize,
            MouseCursor::EResize => wp_cursor_shape_device_v1::Shape::EResize,
            MouseCursor::SeResize => wp_cursor_shape_device_v1::Shape::SeResize,
            MouseCursor::SResize => wp_cursor_shape_device_v1::Shape::SResize,
            MouseCursor::SwResize => wp_cursor_shape_device_v1::Shape::SwResize,
            MouseCursor::WResize => wp_cursor_shape_device_v1::Shape::WResize,
            MouseCursor::NwResize => wp_cursor_shape_device_v1::Shape::NwResize,
            MouseCursor::NsResize => wp_cursor_shape_device_v1::Shape::NsResize,
            MouseCursor::NeswResize => wp_cursor_shape_device_v1::Shape::NeswResize,
            MouseCursor::EwResize => wp_cursor_shape_device_v1::Shape::EwResize,
            MouseCursor::NwseResize => wp_cursor_shape_device_v1::Shape::NwseResize,
            MouseCursor::ColResize => wp_cursor_shape_device_v1::Shape::ColResize,
            MouseCursor::RowResize => wp_cursor_shape_device_v1::Shape::RowResize,
        }
    }
}

pub fn from_mouse(button: u32) -> Option<MouseButton> {
    // ref: linux/input-event-codes.h
    match button {
        0x110 => Some(MouseButton::PRIMARY),
        0x111 => Some(MouseButton::SECONDARY),
        0x112 => Some(MouseButton::MIDDLE),
        0x116 => Some(MouseButton::BACK),
        0x117 => Some(MouseButton::FORWARD),
        _ => None
    }
}
