use {
    std::{
        any::TypeId,
        collections::{HashMap, BTreeSet}
    },
    makepad_math::*,
    //makepad_microserde::*,
    crate::{
       // cx::Cx,
        area::Area,
        //turtle::{Rect, Margin},
        cursor::MouseCursor,
        menu::CommandId,
    },
};

pub const NUM_FINGERS: usize = 10;


#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowGeom {
    pub dpi_factor: f32,
    pub can_fullscreen: bool,
    pub xr_can_present: bool,
    pub xr_is_presenting: bool,
    pub is_fullscreen: bool,
    pub is_topmost: bool,
    pub position: Vec2,
    pub inner_size: Vec2,
    pub outer_size: Vec2,
}


#[derive(Clone, Debug, PartialEq, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool
}
 
#[derive(Clone, Debug, PartialEq)]
pub enum FingerInputType {
    Mouse,
    Touch,
    XR
}

impl FingerInputType {
    pub fn is_touch(&self) -> bool {*self == FingerInputType::Touch}
    pub fn is_mouse(&self) -> bool {*self == FingerInputType::Mouse}
    pub fn is_xr(&self) -> bool {*self == FingerInputType::XR}
    pub fn has_hovers(&self) -> bool {*self == FingerInputType::Mouse || *self == FingerInputType::XR}
}

impl Default for FingerInputType {
    fn default() -> Self {Self::Mouse}
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerDownEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub digit: usize,
    pub tap_count: u32,
    pub handled: bool,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerMoveEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub abs_start: Vec2,
    pub rel: Vec2,
    pub rel_start: Vec2,
    pub rect: Rect,
    pub is_over: bool,
    pub digit: usize,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl FingerMoveEvent {
    pub fn move_distance(&self) -> f32 {
        ((self.abs_start.x - self.abs.x).powf(2.) + (self.abs_start.y - self.abs.y).powf(2.)).sqrt()
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerUpEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub abs_start: Vec2,
    pub rel: Vec2,
    pub rel_start: Vec2,
    pub rect: Rect,
    pub digit: usize,
    pub is_over: bool,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug, PartialEq)]
pub enum HoverState {
    In,
    Over,
    Out
}

impl Default for HoverState {
    fn default() -> HoverState {
        HoverState::Over
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerHoverEvent {
    pub window_id: usize,
    pub digit: usize,
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub any_down: bool,
    pub handled: bool,
    pub hover_state: HoverState,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerScrollEvent {
    pub window_id: usize,
    pub digit: usize,
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub scroll: Vec2,
    pub input_type: FingerInputType,
    //pub is_wheel: bool,
    pub handled_x: bool,
    pub handled_y: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct WindowGeomChangeEvent {
    pub window_id: usize,
    pub old_geom: WindowGeom,
    pub new_geom: WindowGeom,
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct WindowMovedEvent {
    pub window_id: usize,
    pub old_pos: Vec2,
    pub new_pos: Vec2,
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct AnimateEvent {
    pub frame: u64,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct NextFrameEvent {
    pub frame: u64,
    pub time: f64
}
/*
impl NextFrame {
    pub fn is_active(&self, cx: &mut Cx) -> bool {
        cx._next_frames.contains(self)
    }
}*/


#[derive(Clone, Debug, PartialEq)]
pub struct FileReadEvent {
    pub read_id: u64,
    pub data: Result<Vec<u8>, String>
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimerEvent {
    pub timer_id: u64
}

#[derive(Clone, Debug, PartialEq)]
pub struct SignalEvent {
    pub signals: HashMap<Signal, Vec<u64>>
}
/*
#[derive(Clone, Debug, PartialEq)]
pub struct TriggersEvent {
    pub triggers: HashMap<Area, BTreeSet<TriggerId >>
}*/

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerEvent {
    pub triggers: BTreeSet<TriggerId>
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileWriteEvent {
    id: u64,
    error: Option<String>
}
/*
#[derive(Clone, Debug, PartialEq)]
pub struct LiveRecompileEvent {
    pub changed_live_bodies: BTreeSet<LiveBodyId>,
    pub errors: Vec<LiveBodyError>
}*/

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent {
    pub key_code: KeyCode,
    //pub key_char: char,
    pub is_repeat: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyFocusEvent {
    pub prev: Area,
    pub focus: Area,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextInputEvent {
    pub input: String,
    pub replace_last: bool,
    pub was_paste: bool
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextCopyEvent {
    pub response: Option<String>
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowCloseRequestedEvent {
    pub window_id: usize,
    pub accept_close: bool
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowClosedEvent {
    pub window_id: usize
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowResizeLoopEvent {
    pub was_started: bool,
    pub window_id: usize
}

#[derive(Clone, Debug, PartialEq)]
pub enum WindowDragQueryResponse {
    NoAnswer,
    Client,
    Caption,
    SysMenu, // windows only
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowDragQueryEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub response: WindowDragQueryResponse,
}

#[derive(Clone, Debug, Default,  PartialEq)]
pub struct XRButton {
    pub value: f32,
    pub pressed: bool
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct XRInput {
    pub active: bool,
    pub grip: Transform,
    pub ray: Transform,
    pub num_buttons: usize,
    pub buttons: [XRButton; 8],
    pub num_axes: usize,
    pub axes: [f32; 8],
}

#[derive(Clone, Debug, PartialEq)]
pub struct XRUpdateEvent {
    // alright what data are we stuffing in
    pub time: f64,
    pub head_transform: Transform,
    pub left_input: XRInput,
    pub last_left_input: XRInput,
    pub right_input: XRInput,
    pub last_right_input: XRInput,
    pub other_inputs: Vec<XRInput>
}

#[derive(Clone, Debug, PartialEq)]
pub struct WebSocketMessageEvent {
    pub url: String,
    pub result: Result<Vec<u8>, String>
}

#[derive(Clone, Debug, PartialEq)]
pub struct FingerDragEvent {
    pub handled: bool,
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub state: DragState,
    pub action: DragAction,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FingerDropEvent {
    pub handled: bool,
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub dragged_item: DraggedItem,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragState {
    In,
    Over,
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragAction {
    None,
    Copy,
    Link,
    Move,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DraggedItem {
    pub file_urls: Vec<String>
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    None,
    Construct,
    Destruct,
    Draw,
    Paint,
    Redraw,
    AppFocus,
    AppFocusLost,
    //AnimEnded(AnimateEvent),
    //Animate(AnimateEvent),
    NextFrame(NextFrameEvent),
    XRUpdate(XRUpdateEvent),
    WindowSetHoverCursor(MouseCursor),
    WindowDragQuery(WindowDragQueryEvent),
    WindowCloseRequested(WindowCloseRequestedEvent),
    WindowClosed(WindowClosedEvent),
    WindowGeomChange(WindowGeomChangeEvent),
    WindowResizeLoop(WindowResizeLoopEvent),
    FingerDown(FingerDownEvent),
    FingerMove(FingerMoveEvent),
    FingerHover(FingerHoverEvent),
    FingerUp(FingerUpEvent),
    FingerScroll(FingerScrollEvent),
    FileRead(FileReadEvent),
    FileWrite(FileWriteEvent),
    Timer(TimerEvent),
    Signal(SignalEvent),
    //Triggers(TriggersEvent),
    //Trigger(TriggerEvent),
    Command(CommandId),
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy(TextCopyEvent),
    //LiveRecompile(LiveRecompileEvent),
    WebSocketMessage(WebSocketMessageEvent),
    FingerDrag(FingerDragEvent),
    FingerDrop(FingerDropEvent),
    DragEnd,
}

impl Default for Event {
    fn default() -> Event {
        Event::None
    }
}

pub enum HitTouch {
    Single,
    Multi
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Copy, Hash)]
pub struct NextFrame(pub u64);


#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug, Default)]
pub struct Signal {
    pub signal_id: usize
}

impl Signal {
    pub fn empty() -> Signal {
        Signal{signal_id:0}
    }
    
    pub fn is_empty(&self) -> bool {
        self.signal_id == 0
    }
}


// Status


#[derive(PartialEq, Ord, PartialOrd, Copy, Clone, Hash, Eq, Debug)]
pub struct TriggerId(pub TypeId);

impl Into<TriggerId> for TypeId {
    fn into(self) -> TriggerId {TriggerId(self)}
}


#[derive(Clone, Debug, Default)]
pub struct Timer {
    pub timer_id: u64
}

impl Timer {
    pub fn empty() -> Timer {
        Timer {
            timer_id: 0,
        }
    }
    
    pub fn is_empty(&self) -> bool {
        self.timer_id == 0
    }
    
    pub fn is_timer(&mut self, te: &TimerEvent) -> bool {
        te.timer_id == self.timer_id
    }
}

impl Event {
    pub fn set_handled(&mut self, set: bool) {
        match self {
            Event::FingerHover(fe) => {
                fe.handled = set;
            },
            Event::FingerDown(fe) => {
                fe.handled = set;
            },
            _ => ()
        }
    }
    
    pub fn handled(&self) -> bool {
        match self {
            Event::FingerHover(fe) => {
                fe.handled
            },
            Event::FingerDown(fe) => {
                fe.handled
            },
            
            _ => false
        }
    }
    
    
}

// lowest common denominator keymap between desktop and web
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KeyCode {
    Escape,
    
    Backtick,
    Key0,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    Minus,
    Equals,
    
    Backspace,
    Tab,
    
    KeyQ,
    KeyW,
    KeyE,
    KeyR,
    KeyT,
    KeyY,
    KeyU,
    KeyI,
    KeyO,
    KeyP,
    LBracket,
    RBracket,
    Return,
    
    KeyA,
    KeyS,
    KeyD,
    KeyF,
    KeyG,
    KeyH,
    KeyJ,
    KeyK,
    KeyL,
    Semicolon,
    Quote,
    Backslash,
    
    KeyZ,
    KeyX,
    KeyC,
    KeyV,
    KeyB,
    KeyN,
    KeyM,
    Comma,
    Period,
    Slash,
    
    Control,
    Alt,
    Shift,
    Logo,
    
    //RightControl,
    //RightShift,
    //RightAlt,
    //RightLogo,
    
    Space,
    Capslock,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    
    PrintScreen,
    Scrolllock,
    Pause,
    
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    
    NumpadEquals,
    NumpadSubtract,
    NumpadAdd,
    NumpadDecimal,
    NumpadMultiply,
    NumpadDivide,
    Numlock,
    NumpadEnter,
    
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    
    Unknown
}

impl Default for KeyCode {
    fn default() -> Self {KeyCode::Unknown}
}
