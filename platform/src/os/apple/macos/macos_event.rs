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
    PopupDismissed(PopupDismissedEvent),
    WindowGotFocus(WindowId),
    WindowLostFocus(WindowId),
    WindowResizeLoopStart(WindowId),
    WindowResizeLoopStop(WindowId),
    WindowGeomChange(WindowGeomChangeEvent),
    WindowClosed(WindowClosedEvent),
    Paint,

    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    Scroll(ScrollEvent),

    WindowDragQuery(WindowDragQueryEvent),
    WindowCloseRequested(WindowCloseRequestedEvent),
    TextInput(TextInputEvent),
    Drag(DragEvent),
    Drop(DropEvent),
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
