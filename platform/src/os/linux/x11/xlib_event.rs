use crate::{
    event::{
        DragEvent, DropEvent, KeyEvent, MouseDownEvent, MouseLeaveEvent, MouseMoveEvent,
        MouseUpEvent,
        PinchEvent, PopupDismissedEvent, ScrollEvent, TextClipboardEvent, TextInputEvent,
        TimerEvent,
        WindowCloseRequestedEvent, WindowClosedEvent, WindowDragQueryEvent, WindowGeomChangeEvent,
    },
    window::WindowId,
};

#[derive(Debug)]
pub enum XlibEvent {
    WindowGotFocus(WindowId),
    WindowLostFocus(WindowId),
    WindowGeomChange(WindowGeomChangeEvent),
    WindowClosed(WindowClosedEvent),
    PopupDismissed(PopupDismissedEvent),
    Paint,

    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    /// The pointer left the window. Hovered widgets need this to un-hover;
    /// without it the last one stays lit until the pointer comes back.
    MouseLeave(MouseLeaveEvent),
    Scroll(ScrollEvent),
    /// A trackpad pinch; only the Wayland backend produces one.
    Pinch(PinchEvent),

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
}
