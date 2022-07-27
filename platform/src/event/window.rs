use {
    crate::{
        makepad_math::*,
        window::WindowId,
    }
    //makepad_microserde::*,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowGeom {
    pub dpi_factor: f32,
    pub can_fullscreen: bool,
    pub xr_can_present: bool,
    pub xr_is_presenting: bool,
    pub is_fullscreen: bool, 
    pub is_topmost: bool,
    pub position: Vec2,
    pub inner_size: Vec2,
    pub outer_size: Vec2,
}
 

#[derive(Clone, Debug, PartialEq)]
pub struct WindowGeomChangeEvent {
    pub window_id: WindowId,
    pub old_geom: WindowGeom,
    pub new_geom: WindowGeom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowMovedEvent {
    pub window_id: WindowId,
    pub old_pos: Vec2,
    pub new_pos: Vec2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowCloseRequestedEvent {
    pub window_id: WindowId,
    pub accept_close: bool
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowClosedEvent {
    pub window_id: WindowId
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowResizeLoopEvent {
    pub was_started: bool,
    pub window_id: WindowId
}

#[derive(Clone, Debug, PartialEq)]
pub enum WindowDragQueryResponse {
    NoAnswer,
    Client,
    Caption,
    SysMenu, // windows only
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowDragQueryEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub response: WindowDragQueryResponse,
}
