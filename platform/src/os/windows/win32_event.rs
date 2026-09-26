use crate::{
    event::{
        DragEvent, DropEvent, KeyEvent, MouseDownEvent, MouseLeaveEvent, MouseMoveEvent,
        MouseUpEvent, PopupDismissedEvent, ScrollEvent, TextClipboardEvent, TextInputEvent,
        TimerEvent, WindowCloseRequestedEvent, WindowClosedEvent, WindowDragQueryEvent,
        WindowGeomChangeEvent,
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
    PopupDismissed(PopupDismissedEvent),
    Paint,
    /// One window's DXGI frame-latency waitable was signaled: the compositor
    /// retired a present and is ready for the next frame of THAT window. This
    /// is the Windows twin of macOS's `MacosEvent::LinkFire` — it carries which
    /// window flipped and the app-time it woke at. The beat paints that
    /// window's pass tree; it also steps the app clock (the whole tick) when
    /// no beat has done so for the flip it aims at (`windows.rs`).
    Beat {
        window_id: WindowId,
        time: f64,
    },

    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    MouseLeave(MouseLeaveEvent),
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
    Signal,
}
