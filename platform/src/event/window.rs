use {
    std::rc::Rc,
    std::cell::Cell,
    crate::{
        makepad_math::*,
        window::WindowId,
    }
    //makepad_microserde::*,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowGeom {
    pub dpi_factor: f64,
    pub can_fullscreen: bool,
    pub xr_can_present: bool,
    pub xr_is_presenting: bool,
    pub is_fullscreen: bool, 
    pub is_topmost: bool,
    pub position: DVec2,
    pub inner_size: DVec2,
    pub outer_size: DVec2,
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
    pub old_pos: DVec2,
    pub new_pos: DVec2,
}

#[derive(Clone, Debug)]
pub struct WindowCloseRequestedEvent {
    pub window_id: WindowId,
    pub accept_close: Rc<Cell<bool>>
}

#[derive(Clone, Debug)]
pub struct WindowClosedEvent {
    pub window_id: WindowId
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
    pub abs: DVec2,
    pub response: Rc<Cell<WindowDragQueryResponse>>,
}
