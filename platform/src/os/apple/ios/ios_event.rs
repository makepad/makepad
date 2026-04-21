use crate::{
    event::{
        KeyEvent, LongPressEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollEvent,
        SelectionHandleDragEvent, TextClipboardEvent, TextInputEvent, TextRangeReplaceEvent,
        TimerEvent, TouchUpdateEvent, VirtualKeyboardEvent, WindowGeomChangeEvent,
    },
    permission::PermissionResult,
    window::WindowId,
};

#[derive(Debug, Clone)]
pub enum IosEvent {
    Init,
    WindowGotFocus(WindowId),
    WindowLostFocus(WindowId),
    WindowGeomChange(WindowGeomChangeEvent),
    Paint,
    VirtualKeyboard(VirtualKeyboardEvent),
    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    TouchUpdate(TouchUpdateEvent),
    LongPress(LongPressEvent),

    Scroll(ScrollEvent),

    TextInput(TextInputEvent),
    TextRangeReplace(TextRangeReplaceEvent),
    SelectionHandleDrag(SelectionHandleDragEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextCopy(TextClipboardEvent),
    TextCut(TextClipboardEvent),
    Timer(TimerEvent),
    PermissionResult(PermissionResult),
}
