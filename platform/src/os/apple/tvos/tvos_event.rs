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
    AppGotFocus(WindowId),
    AppLostFocus(WindowId),
    WindowGeomChange(WindowGeomChangeEvent),
    Paint,
    Timer(TimerEvent),
}
