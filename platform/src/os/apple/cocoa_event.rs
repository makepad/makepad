use {
    std::cell::Cell,
    crate::{
        makepad_math::DVec2,
        area::Area,
        window::WindowId,
        menu::MenuCommand,
        event::{
            MouseDownEvent,
            MouseUpEvent,
            MouseMoveEvent,
            ScrollEvent,
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

impl From<CocoaMouseDownEvent> for MouseDownEvent {
    fn from(v: CocoaMouseDownEvent) -> Self {
        Self{
            abs: v.abs,
            button: v.button,
            window_id: v.window_id,
            modifiers: v.modifiers,
            time: v.time,
            handled: Cell::new(Area::Empty),
            sweep_lock: Cell::new(Area::Empty),
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

impl From<CocoaMouseMoveEvent> for MouseMoveEvent {
    fn from(v: CocoaMouseMoveEvent) -> Self {
        Self{
            abs: v.abs,
            window_id: v.window_id,
            modifiers: v.modifiers,
            time: v.time,
            handled: Cell::new(Area::Empty),
            sweep_lock: Cell::new(Area::Empty),
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

impl From<CocoaMouseUpEvent> for MouseUpEvent {
    fn from(v: CocoaMouseUpEvent) -> Self {
        Self{
            abs: v.abs,
            button: v.button,
            window_id: v.window_id,
            modifiers: v.modifiers,
            time: v.time,
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

impl From<CocoaScrollEvent> for ScrollEvent {
    fn from(v: CocoaScrollEvent) -> Self {
        Self{
            abs: v.abs,
            scroll: v.scroll,
            window_id: v.window_id,
            modifiers: v.modifiers,
            sweep_lock: Cell::new(Area::Empty),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: true,
            time: v.time,
        }
    }
}
