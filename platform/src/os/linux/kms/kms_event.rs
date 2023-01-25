use {
    crate::{
        event::{
            MouseDownEvent,
            MouseUpEvent,
            MouseMoveEvent,
            ScrollEvent,
            KeyEvent,
            TimerEvent,
        },
    }
};

#[derive(Debug)]
pub enum KmsEvent {
    Paint,
    MouseDown(MouseDownEvent),
    MouseUp(MouseUpEvent),
    MouseMove(MouseMoveEvent),
    Scroll(ScrollEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    Timer(TimerEvent),
}
