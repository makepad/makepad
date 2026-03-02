use crate::{
    event::{
        DragEvent, DropEvent, KeyEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        PopupDismissedEvent, ScrollEvent, TextClipboardEvent, TextInputEvent, TimerEvent,
        WindowCloseRequestedEvent,
        WindowClosedEvent, WindowDragQueryEvent, WindowGeomChangeEvent,
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
}
