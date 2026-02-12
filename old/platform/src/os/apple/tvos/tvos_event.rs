use crate::{
    event::{TimerEvent, WindowGeomChangeEvent},
    window::WindowId,
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
