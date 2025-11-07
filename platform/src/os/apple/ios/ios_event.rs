use {
    crate::{
        window::WindowId,
        event::{
            MouseDownEvent,
            MouseUpEvent,
            MouseMoveEvent,
            ScrollEvent,
            WindowGeomChangeEvent,
            TextInputEvent,
            KeyEvent,
            TextClipboardEvent,
            TimerEvent,
            LongPressEvent,
            TouchUpdateEvent,
            VirtualKeyboardEvent,
        },
        permission::PermissionResult,
    }
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
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextCopy(TextClipboardEvent),
    TextCut(TextClipboardEvent),
    Timer(TimerEvent),
    PermissionResult(PermissionResult),
}
