use {
    crate::{
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
            TouchUpdateEvent,
            VirtualKeyboardEvent,
        },
    }
};

#[derive(Debug, Clone)]
pub enum IosEvent {
    Init,
    AppGotFocus,
    AppLostFocus,
    WindowGeomChange(WindowGeomChangeEvent),
    Paint,
    VirtualKeyboard(VirtualKeyboardEvent),
    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    TouchUpdate(TouchUpdateEvent),
    
    Scroll(ScrollEvent),
    
    TextInput(TextInputEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextCopy(TextClipboardEvent),
    TextCut(TextClipboardEvent),
    Timer(TimerEvent),
}
