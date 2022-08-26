use {
    std::cell::Cell,
    crate::{
        makepad_math::DVec2,
        area::Area,
        window::WindowId,
        menu::MenuCommand,
        event::{
            CxFingers,
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
    pub abs: DVec2,
    pub button: usize,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl CocoaMouseDownEvent {
    pub fn into_finger_down_event(self, fingers: &CxFingers, digit_id: DigitId) -> FingerDownEvent {
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

#[derive(Debug)]
pub struct CocoaMouseMoveEvent {
    pub abs: DVec2,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl CocoaMouseMoveEvent {
    pub fn into_finger_hover_event(self, digit_id: DigitId, hover_last: Area, button: usize) -> FingerHoverEvent {
        FingerHoverEvent {
            window_id: self.window_id,
            abs: self.abs,
            digit_id,
            hover_last,
            handled: Cell::new(false),
            sweep_lock: Cell::new(Area::Empty),
            device: DigitDevice::Mouse(button),
            modifiers: self.modifiers,
            time: self.time
        }
    }
    pub fn into_finger_move_event(self, fingers: &CxFingers, digit_id: DigitId, button: usize) -> FingerMoveEvent {
        FingerMoveEvent {
            window_id: self.window_id,
            handled: Cell::new(Area::Empty),
            sweep_lock: Cell::new(Area::Empty),
            hover_last: fingers.get_hover_area(digit_id), 
            tap_count: fingers.get_tap_count(digit_id),
            abs: self.abs,
            digit: DigitInfo {
                id: digit_id,
                index: fingers.get_digit_index(digit_id),
                count: fingers.get_digit_count(),
                device: DigitDevice::Mouse(button),
            },
            modifiers: self.modifiers,
            time: self.time
        }
    }
}

#[derive(Debug)]
pub struct CocoaMouseUpEvent {
    pub abs: DVec2,
    pub button: usize,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl CocoaMouseUpEvent {
    pub fn into_finger_up_event(self, fingers: &CxFingers, digit_id: DigitId) -> FingerUpEvent {
        FingerUpEvent {
            window_id: self.window_id,
            abs: self.abs,
            digit: DigitInfo {
                id: digit_id,
                index: fingers.get_digit_index(digit_id),
                count: fingers.get_digit_count(),
                device: DigitDevice::Mouse(self.button),
            },
            capture_time: fingers.get_capture_time(digit_id),
            tap_count: fingers.get_tap_count(digit_id), 
            captured: fingers.get_captured_area(digit_id),
            modifiers: self.modifiers,
            time: self.time
        }
    }
}

#[derive(Debug)]
pub struct CocoaScrollEvent {
    pub window_id: WindowId,
    pub scroll: DVec2,
    pub abs: DVec2,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl CocoaScrollEvent {
    pub fn into_finger_scroll_event(self, digit_id: DigitId) -> FingerScrollEvent {
        FingerScrollEvent {
            window_id: self.window_id,
            abs: self.abs,
            digit_id,
            sweep_lock: Cell::new(Area::Empty),
            scroll: self.scroll,
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            device: DigitDevice::Mouse(0),
            modifiers: self.modifiers,
            time: self.time
        }
    }
}
