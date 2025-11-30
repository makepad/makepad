use {
    crate::{
        window::WindowId,
        event::{
            WindowGeomChangeEvent,
            TimerEvent,
        },
    }
};

#[derive(Debug, Clone)]
pub enum TvosEvent {
    Init,
    WindowGotFocus(WindowId),
    WindowLostFocus(WindowId),
    WindowGeomChange(WindowGeomChangeEvent),
    Paint,
    Timer(TimerEvent),
}
