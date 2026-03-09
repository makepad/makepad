use {
    crate::{makepad_math::*, window::WindowId}, //makepad_microserde::*,
    std::cell::Cell,
    std::rc::Rc,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowGeom {
    pub dpi_factor: f64,
    pub can_fullscreen: bool,
    pub xr_is_presenting: bool,
    pub is_fullscreen: bool,
    pub is_topmost: bool,
    pub position: Vec2d,
    pub inner_size: Vec2d,
    pub outer_size: Vec2d,
}

#[derive(Clone, Debug)]
pub struct WindowGeomChangeEvent {
    pub window_id: WindowId,
    pub old_geom: WindowGeom,
    pub new_geom: WindowGeom,
}

#[derive(Clone, Debug)]
pub struct WindowMovedEvent {
    pub window_id: WindowId,
    pub old_pos: Vec2d,
    pub new_pos: Vec2d,
}

#[derive(Clone, Debug)]
pub struct WindowCloseRequestedEvent {
    pub window_id: WindowId,
    pub accept_close: Rc<Cell<bool>>,
}

#[derive(Clone, Debug)]
pub struct WindowClosedEvent {
    pub window_id: WindowId,
}

#[derive(Clone, Debug)]
pub enum PopupDismissReason {
    FocusLost,
    OutsideClick,
    Escape,
    Compositor,
    ParentClosed,
}

/// Notification that a popup window should be closed.
///
/// The app **must** call `WindowHandle::close()` to actually close the popup.
/// The framework does not auto-close popup windows on dismissal.
///
/// On Wayland the compositor may force-close the surface (`PopupDone`); in
/// that case `PopupDismissed` fires after the surface is already gone.
///
/// Common reasons: `OutsideClick`, `FocusLost`, `Escape`, `Compositor`,
/// `ParentClosed`.
#[derive(Clone, Debug)]
pub struct PopupDismissedEvent {
    pub window_id: WindowId,
    pub reason: PopupDismissReason,
}
/*
#[derive(Clone, Debug)]
pub struct WindowResizeLoopEvent {
    pub was_started: bool,
    pub window_id: WindowId
}*/

#[derive(Clone, Debug, Copy)]
pub enum WindowDragQueryResponse {
    NoAnswer,
    Client,
    Caption,
    SysMenu, // windows only
}

#[derive(Clone, Debug)]
pub struct WindowDragQueryEvent {
    pub window_id: WindowId,
    pub abs: Vec2d,
    pub response: Rc<Cell<WindowDragQueryResponse>>,
}
