use {
    crate::{
        event::{
            WindowGeomChangeEvent,
            TimerEvent,
        },
    }
};

#[derive(Debug, Clone)]
pub enum TvosEvent {
    Init,
    AppGotFocus,
    AppLostFocus,
    WindowGeomChange(WindowGeomChangeEvent),
    Paint,
    Timer(TimerEvent),
}
