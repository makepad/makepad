use {
    crate::{
        window::WindowId,
        event::{
            WindowGeomChangeEvent,
            TimerEvent,
            GamepadConnectedEvent
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
    GamepadConnected(GamepadConnectedEvent),
}
