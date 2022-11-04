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
            TextCopyEvent,
            TimerEvent,
            SignalEvent,
        },
    }
};

#[derive(Debug)]
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
    TextCopy(TextCopyEvent),
    Timer(TimerEvent),
    Signal(SignalEvent),
    MenuCommand(MenuCommand),
}
/*impl CocoaMouseDownEvent {
    pub fn into_mouse_down_event(self, fingers: &CxFingers, digit_id: DigitId) -> FingerDownEvent {
        FingerDownEvent {
            window_id: self.window_id,
            abs: self.abs,
            digit: DigitInfo {
                id: digit_id,
                index: fingers.get_digit_index(digit_id),
                count: fingers.get_digit_count(),
                device: DigitDevice::Mouse(self.button),
            },
            sweep_lock: Cell::new(Area::Empty),
            tap_count: fingers.get_tap_count(digit_id),
            handled: Cell::new(Area::Empty),
            modifiers: self.modifiers,
            time: self.time
        }
    }
}
*/
