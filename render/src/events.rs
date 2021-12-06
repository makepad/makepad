use {
    std::{
        any::TypeId,
        collections::HashMap
    },
    makepad_math::*,
    //makepad_microserde::*,
    crate::{
        cx::Cx,
        turtle::{Margin},
        area::Area,
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
    pub digit: usize,
    pub tap_count: u32,
    pub handled: bool,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerDownHitEvent {
    pub rel: Vec2,
    pub rect: Rect,
    pub event: FingerDownEvent
}

impl std::ops::Deref for FingerDownHitEvent{
    type Target = FingerDownEvent;
    fn deref(&self) -> &Self::Target {&self.event}
}

impl std::ops::DerefMut for FingerDownHitEvent{
    fn deref_mut(&mut self) -> &mut  Self::Target {&mut self.event}
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerMoveEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub digit: usize,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerMoveHitEvent {
    pub abs_start: Vec2,
    pub rel: Vec2,
    pub rel_start: Vec2,
    pub rect: Rect,
    pub is_over: bool,
    pub event: FingerMoveEvent,
}

impl std::ops::Deref for FingerMoveHitEvent{
    type Target = FingerMoveEvent;
    fn deref(&self) -> &Self::Target {&self.event}
}

impl std::ops::DerefMut for FingerMoveHitEvent{
    fn deref_mut(&mut self) -> &mut  Self::Target {&mut self.event}
}


impl FingerMoveHitEvent {
    pub fn move_distance(&self) -> f32 {
        ((self.abs_start.x - self.abs.x).powf(2.) + (self.abs_start.y - self.abs.y).powf(2.)).sqrt()
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerUpEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub digit: usize,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerUpHitEvent {
    pub rel: Vec2,
    pub abs_start: Vec2,
    pub rel_start: Vec2,
    pub rect: Rect,
    pub is_over: bool,
    pub event: FingerUpEvent
}

impl std::ops::Deref for FingerUpHitEvent{
    type Target = FingerUpEvent;
    fn deref(&self) -> &Self::Target {&self.event}
}

impl std::ops::DerefMut for FingerUpHitEvent{
    fn deref_mut(&mut self) -> &mut  Self::Target {&mut self.event}
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
    pub handled: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerHoverHitEvent {
    pub rel: Vec2,
    pub rect: Rect,
    pub any_down: bool,
    pub hover_state: HoverState,
    pub event: FingerHoverEvent
}

impl std::ops::Deref for FingerHoverHitEvent{
    type Target = FingerHoverEvent;
    fn deref(&self) -> &Self::Target {&self.event}
}

impl std::ops::DerefMut for FingerHoverHitEvent{
    fn deref_mut(&mut self) -> &mut  Self::Target {&mut self.event}
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerScrollEvent {
    pub window_id: usize,
    pub digit: usize,
    pub abs: Vec2,
    pub scroll: Vec2,
    pub input_type: FingerInputType,
    pub handled_x: bool,
    pub handled_y: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerScrollHitEvent {
    pub rel: Vec2,
    pub rect: Rect,
    pub event: FingerScrollEvent
}

impl std::ops::Deref for FingerScrollHitEvent{
    type Target = FingerScrollEvent;
    fn deref(&self) -> &Self::Target {&self.event}
}

impl std::ops::DerefMut for FingerScrollHitEvent{
    fn deref_mut(&mut self) -> &mut  Self::Target {&mut self.event}
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
pub struct TimerEvent {
    pub timer_id: u64
}

#[derive(Clone, Debug, PartialEq)]
pub struct SignalEvent {
    pub signals: HashMap<Signal, Vec<u64>>
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent {
    pub key_code: KeyCode,
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

#[derive(Debug, PartialEq)]
pub struct FingerDragHitEvent<'a> {
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub state: DragState,
    pub action: &'a mut DragAction,
}

#[derive(Debug, PartialEq)]
pub struct FingerDropHitEvent<'a> {
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub dragged_item: &'a mut DraggedItem,
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
    Timer(TimerEvent),
    Signal(SignalEvent),
    Command(CommandId),
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy(TextCopyEvent),
    WebSocketMessage(WebSocketMessageEvent),
    FingerDrag(FingerDragEvent),
    FingerDrop(FingerDropEvent),
    DragEnd,
}

pub enum HitEvent<'a>{
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy(&'a mut TextCopyEvent),
    FingerScroll(FingerScrollHitEvent),
    FingerDown(FingerDownHitEvent),
    FingerMove(FingerMoveHitEvent),
    FingerHover(FingerHoverHitEvent),
    FingerUp(FingerUpHitEvent),
    None
}

pub enum DragEvent<'a>{
    FingerDrag(FingerDragHitEvent<'a>),
    FingerDrop(FingerDropHitEvent<'a>),
    DragEnd,
    None
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


#[derive(Copy, Clone, Debug, Default)]
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


#[derive(Clone, Debug, Default)]
pub struct HitOpt {
    pub use_multi_touch: bool,
    pub margin: Option<Margin>,
}

pub fn rect_contains_with_margin(rect:&Rect, pos: Vec2, margin: &Option<Margin>) -> bool {
    if let Some(margin) = margin {
        return
        pos.x >= rect.pos.x - margin.l
            && pos.x <= rect.pos.x + rect.size.x + margin.r
            && pos.y >= rect.pos.y - margin.t
            && pos.y <= rect.pos.y + rect.size.y + margin.b;
    }
    else {
        return rect.contains(pos);
    }
}

impl Event {
    
    pub fn is_next_frame(&self, cx: &mut Cx, next_frame: NextFrame) -> Option<NextFrameEvent> {
        match self {
            Event::NextFrame(fe) => {
                if cx._next_frames.contains(&next_frame) {
                    return Some(fe.clone())
                }
            }
            _ => ()
        }
        None
    }
    
    pub fn is_timer(&self, timer: Timer) -> bool{
        match self {
            Event::Timer(te) => {
                return te.timer_id == timer.timer_id
            }
            _ => ()
        }
        false
    }

    pub  fn hits(&mut self, cx: &mut Cx, area: Area, opt: HitOpt) -> HitEvent {
        match self {
            Event::KeyFocus(kf) => {
                if area == kf.prev {
                    return HitEvent::KeyFocusLost(kf.clone())
                }
                else if area == kf.focus {
                    return HitEvent::KeyFocus(kf.clone())
                }
            },
            Event::KeyDown(kd) => {
                if area == cx.key_focus {
                    return HitEvent::KeyDown(kd.clone())
                }
            },
            Event::KeyUp(ku) => {
                if area == cx.key_focus {
                    return HitEvent::KeyUp(ku.clone())
                }
            },
            Event::TextInput(ti) => {
                if area == cx.key_focus {
                    return HitEvent::TextInput(ti.clone())
                }
            },
            Event::TextCopy(tc) => {
                if area == cx.key_focus {
                    return HitEvent::TextCopy(tc);
                }
            },
            Event::FingerScroll(fe) => {
                let rect = area.get_rect(&cx);
                if rect_contains_with_margin(&rect, fe.abs, &opt.margin) {
                    //fe.handled = true;
                    return HitEvent::FingerScroll(FingerScrollHitEvent {
                        rel: fe.abs - rect.pos,
                        rect: rect,
                        event: fe.clone()
                    })
                }
            },
            Event::FingerHover(fe) => {
                let rect = area.get_rect(&cx);
                
                if cx.fingers[fe.digit]._over_last == area {
                    let mut any_down = false;
                    for finger in &cx.fingers {
                        if finger.captured == area {
                            any_down = true;
                            break;
                        }
                    }
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &opt.margin) {
                        fe.handled = true;
                        //if let HoverState::Out = fe.hover_state {
                            //    cx.finger_over_last_area = Area::Empty;
                        //}
                        //else {
                            cx.fingers[fe.digit].over_last = area;
                        // }
                        return HitEvent::FingerHover(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            hover_state: HoverState::Over,
                            event:fe.clone()
                        })
                    }
                    else {
                        //self.was_over_last_call = false;
                        return HitEvent::FingerHover(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            hover_state: HoverState::Out,
                            event:fe.clone()
                        })
                    }
                }
                else {
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &opt.margin) {
                        let mut any_down = false;
                        for finger in &cx.fingers {
                            if finger.captured == area {
                                any_down = true;
                                break;
                            }
                        }
                        cx.fingers[fe.digit].over_last = area;
                        fe.handled = true;
                        //self.was_over_last_call = true;
                        return HitEvent::FingerHover(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            hover_state: HoverState::In,
                            event:fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe) => {
                // check wether our digit is captured, otherwise don't send
                if cx.fingers[fe.digit].captured == area {
                    let abs_start = cx.fingers[fe.digit].down_abs_start;
                    let rel_start = cx.fingers[fe.digit].down_rel_start;
                    let rect = area.get_rect(&cx);
                    return HitEvent::FingerMove(FingerMoveHitEvent {
                        abs_start: abs_start,
                        rel: area.abs_to_rel(cx, fe.abs),
                        rel_start: rel_start,
                        rect: rect,
                        is_over: rect_contains_with_margin(&rect, fe.abs, &opt.margin),
                        event:fe.clone()
                    })
                }
            },
            Event::FingerDown(fe) => {
                if !fe.handled {
                    let rect = area.get_rect(&cx);
                    if rect_contains_with_margin(&rect, fe.abs, &opt.margin) {
                        // scan if any of the fingers already captured this area
                        if !opt.use_multi_touch {
                            for finger in &cx.fingers {
                                if finger.captured == area {
                                    return HitEvent::None;
                                }
                            }
                        }
                        cx.fingers[fe.digit].captured = area;
                        let rel = area.abs_to_rel(cx, fe.abs);
                        cx.fingers[fe.digit].down_abs_start = fe.abs;
                        cx.fingers[fe.digit].down_rel_start = rel;
                        fe.handled = true;
                        return HitEvent::FingerDown(FingerDownHitEvent {
                            rel: rel,
                            rect: rect,
                            event:fe.clone()
                        })
                    }
                }
            },
            Event::FingerUp(fe) => {
                if cx.fingers[fe.digit].captured == area {
                    cx.fingers[fe.digit].captured = Area::Empty;
                    let abs_start = cx.fingers[fe.digit].down_abs_start;
                    let rel_start = cx.fingers[fe.digit].down_rel_start;
                    let rect = area.get_rect(&cx);
                    return HitEvent::FingerUp(FingerUpHitEvent {
                        is_over: rect.contains(fe.abs),
                        abs_start: abs_start,
                        rel_start: rel_start,
                        rel: area.abs_to_rel(cx, fe.abs),
                        rect: rect,
                        event:fe.clone()
                    })
                }
            },
            _ => ()
        };
        HitEvent::None
    }
    
    pub fn drag_hits(&mut self, cx: &mut Cx, area: Area, opt: HitOpt) -> DragEvent {
        match self {
            Event::FingerDrag(event) => {
                let rect = area.get_rect(cx);
                if area == cx.drag_area {
                    if !event.handled && rect_contains_with_margin(&rect, event.abs, &opt.margin) {
                        cx.new_drag_area = area;
                        event.handled = true;
                        DragEvent::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            abs: event.abs,
                            state: event.state.clone(),
                            action: &mut event.action
                        })
                    } else {
                        DragEvent::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            state: DragState::Out,
                            abs: event.abs,
                            action: &mut event.action
                        })
                    }
                } else {
                    if !event.handled && rect_contains_with_margin(&rect, event.abs, &opt.margin) {
                        cx.new_drag_area = area;
                        event.handled = true;
                        DragEvent::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            state: DragState::In,
                            abs: event.abs,
                            action: &mut event.action
                        })
                    } else {
                        DragEvent::None
                    }
                }
            }
            Event::FingerDrop(event) => {
                let rect = area.get_rect(cx);
                if !event.handled && rect_contains_with_margin(&rect, event.abs, &opt.margin) {
                    cx.new_drag_area = Area::default();
                    event.handled = true;
                    DragEvent::FingerDrop(FingerDropHitEvent {
                        rel: area.abs_to_rel(cx, event.abs),
                        rect,
                        abs: event.abs,
                        dragged_item: &mut event.dragged_item
                    })
                } else {
                    DragEvent::None
                }
            }
            _ => DragEvent::None,
        }
    }
    
}

