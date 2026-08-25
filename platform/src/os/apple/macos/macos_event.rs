use crate::{
    event::window::PopupDismissedEvent,
    event::{
        DragEvent, DropEvent, GameInputConnectedEvent, KeyEvent, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, ScrollEvent, TextClipboardEvent, TextInputEvent, TimerEvent,
        WindowCloseRequestedEvent, WindowClosedEvent, WindowDragQueryEvent, WindowGeomChangeEvent,
    },
    makepad_live_id::*,
    permission::PermissionResult,
    window::WindowId,
};

#[derive(Debug, Clone)]
pub enum MacosEvent {
    AppQuitRequested,
    PopupDismissed(PopupDismissedEvent),
    WindowGotFocus(WindowId),
    WindowLostFocus(WindowId),
    WindowResizeLoopStart(WindowId),
    WindowResizeLoopStop(WindowId),
    WindowGeomChange(WindowGeomChangeEvent),
    WindowClosed(WindowClosedEvent),
    Paint,
    /// One window's display link fired: paint THAT window at its own flip,
    /// with the flip's target timestamp (app-time domain). CAMetalDisplayLink
    /// also supplies the drawable for that flip and its Core Animation target
    /// presentation time; CADisplayLink leaves `drawable` as None. The primary
    /// link (index 0) also runs the shared logic beat.
    LinkFire {
        window: crate::os::apple::apple_sys::ObjcId,
        time: f64,
        primary: bool,
        drawable: Option<crate::os::apple::apple_sys::ObjcId>,
        target_presentation_time: f64,
    },

    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    Scroll(ScrollEvent),

    WindowDragQuery(WindowDragQueryEvent),
    WindowCloseRequested(WindowCloseRequestedEvent),
    TextInput(TextInputEvent),
    Drag(WindowId, DragEvent),
    Drop(WindowId, DropEvent),
    DragEnd,
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextCopy(TextClipboardEvent),
    TextCut(TextClipboardEvent),
    Timer(TimerEvent),
    MacosMenuCommand(LiveId),
    PermissionResult(PermissionResult),
    GameInputConnected(GameInputConnectedEvent),
}
