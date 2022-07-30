use {
    std::cell::Cell,
    crate::{
        makepad_math::Vec2,
        area::Area,
        window::WindowId,
        menu::MenuCommand,
        event::{
            DigitId,
            DigitDevice,
            DigitInfo,
            FingerDownEvent,
            FingerUpEvent,
            FingerHoverEvent,
            FingerMoveEvent,
            FingerScrollEvent,
            WindowGeomChangeEvent,
            WindowDragQueryEvent,
            KeyModifiers,
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
    
    MouseDown(CocoaMouseDownEvent),
    MouseUp(CocoaMouseUpEvent),
    MouseMove(CocoaMouseMoveEvent),
    Scroll(CocoaScrollEvent),
    
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

#[derive(Debug)]
pub struct CocoaMouseDownEvent {
    pub abs: Vec2,
    pub button: usize,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl CocoaMouseDownEvent {
    pub fn into_finger_down_event(self, digit_id: DigitId, digit_index:usize, digit_count:usize, tap_count: u32) -> FingerDownEvent {
        FingerDownEvent {
            window_id: self.window_id,
            abs: self.abs,
            digit: DigitInfo{
                id:digit_id,
                index:digit_index,
                count:digit_count,
                device: DigitDevice::Mouse(self.button),
            },
            tap_count,
            handled: Cell::new(false),
            modifiers: self.modifiers,
            time: self.time
        }
    }
}

#[derive(Debug)]
pub struct CocoaMouseMoveEvent {
    pub abs: Vec2,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl CocoaMouseMoveEvent {
    pub fn into_finger_hover_event(self, digit_id: DigitId,  hover_last: Area, button: usize) -> FingerHoverEvent {
        FingerHoverEvent {
            window_id: self.window_id,
            abs: self.abs,
            digit_id,
            hover_last,
            handled: Cell::new(false),
            device: DigitDevice::Mouse(button),
            modifiers: self.modifiers,
            time: self.time
        }
    }
    pub fn into_finger_move_event(self, digit_id: DigitId, digit_index:usize, digit_count:usize, captured: Area, button: usize) -> FingerMoveEvent {
        FingerMoveEvent {
            window_id: self.window_id,
            captured,
            abs: self.abs,
            digit: DigitInfo{
                id:digit_id,
                index:digit_index,
                count:digit_count,
                device: DigitDevice::Mouse(button),
            },
            modifiers: self.modifiers,
            time: self.time
        }
    }
}

#[derive(Debug)]
pub struct CocoaMouseUpEvent {
    pub abs: Vec2,
    pub button: usize,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl CocoaMouseUpEvent {
    pub fn into_finger_up_event(self, digit_id: DigitId, digit_index:usize, digit_count:usize, captured: Area) -> FingerUpEvent {
        FingerUpEvent {
            window_id: self.window_id,
            abs: self.abs,
            digit: DigitInfo{
                id:digit_id,
                index:digit_index,
                count:digit_count,
                device: DigitDevice::Mouse(self.button),
            },
            captured,
            modifiers: self.modifiers,
            time: self.time
        }
    }
}

#[derive(Debug)]
pub struct CocoaScrollEvent {
    pub window_id: WindowId,
    pub scroll: Vec2,
    pub abs: Vec2,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl CocoaScrollEvent {
    pub fn into_finger_scroll_event(self, digit_id: DigitId) -> FingerScrollEvent {
        FingerScrollEvent {
            window_id: self.window_id,
            abs: self.abs,
            digit_id,
            scroll: self.scroll,
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            device: DigitDevice::Mouse(0),
            modifiers: self.modifiers,
            time: self.time
        }
    }
}
