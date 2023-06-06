use {
    crate::{
        window::WindowId,
        menu::MenuCommand,
        event::{
            MouseDownEvent,
            MouseUpEvent,
            MouseMoveEvent,
            ScrollEvent,
            WindowGeomChangeEvent,
            WindowDragQueryEvent,
            WindowCloseRequestedEvent,
            WindowClosedEvent,
            TextInputEvent,
            KeyEvent,
            DragEvent,
            DropEvent,
            TextClipboardEvent,
            TimerEvent,
        },
    }
};

#[derive(Debug, Clone)]
pub enum CocoaEvent {
    AppGotFocus,
    AppLostFocus,
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
    MenuCommand(MenuCommand),
}
