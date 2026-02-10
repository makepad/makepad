use crate::{
    event::{
        DragEvent, DropEvent, KeyEvent, MouseDownEvent, MouseLeaveEvent, MouseMoveEvent,
        MouseUpEvent, ScrollEvent, TextClipboardEvent, TextInputEvent, TimerEvent,
        WindowCloseRequestedEvent, WindowClosedEvent, WindowDragQueryEvent, WindowGeomChangeEvent,
    },
    window::WindowId,
};

#[derive(Debug)]
pub enum Win32Event {
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
    MouseLeave(MouseLeaveEvent),
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
    Signal,
}
