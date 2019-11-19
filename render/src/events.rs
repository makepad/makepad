use crate::cx::*;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool
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
    pub is_touch: bool,
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
    pub is_touch: bool,
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
    pub is_touch: bool,
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
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub scroll: Vec2,
    pub is_wheel: bool,
    pub handled: bool,
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
pub struct FrameEvent {
    pub frame: u64,
    pub time: f64
}

#[derive(Clone, Default, Debug)]
pub struct RedrawEvent {
    pub area: Area
}

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
    pub signal_id: usize,
    pub value: usize
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileWriteEvent {
    id: u64,
    error: Option<String>
}

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
    pub last: Area,
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

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    None,
    Construct,
    Destruct,
    Draw,
    Paint,
    AppFocus,
    AppFocusLost,
    AnimEnded(AnimateEvent),
    Animate(AnimateEvent),
    Frame(FrameEvent),
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
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy(TextCopyEvent)
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

#[derive(Clone, Debug, Default)]
pub struct HitOpt {
    pub use_multi_touch: bool,
    pub no_scrolling: bool,
    pub margin: Option<Margin>,
}

impl Event {
    
    pub fn hits(&mut self, cx: &mut Cx, area: Area, opt: HitOpt) -> Event {
        match self {
            Event::KeyFocus(kf) => {
                if area == kf.last {
                    return Event::KeyFocusLost(kf.clone())
                }
                else if area == kf.focus {
                    return Event::KeyFocus(kf.clone())
                }
            },
            Event::KeyDown(_) => {
                if area == cx.key_focus {
                    return self.clone();
                }
            },
            Event::KeyUp(_) => {
                if area == cx.key_focus {
                    return self.clone();
                }
            },
            Event::TextInput(_) => {
                if area == cx.key_focus {
                    return self.clone();
                }
            },
            Event::TextCopy(_) => {
                if area == cx.key_focus {
                    return Event::TextCopy(
                        TextCopyEvent {response: None}
                    );
                }
            },
            Event::Animate(_) => {
                for anim in &cx.playing_anim_areas {
                    if anim.area == area {
                        return self.clone()
                    }
                }
            },
            Event::Frame(_) => {
                for frame_area in &cx._frame_callbacks {
                    if *frame_area == area {
                        return self.clone()
                    }
                }
            },
            Event::AnimEnded(_) => {
                for anim in &cx.ended_anim_areas {
                    if anim.area == area {
                        return self.clone()
                    }
                }
            },
            Event::FingerScroll(fe) => {
                let rect = area.get_rect(&cx, opt.no_scrolling);
                if !fe.handled && rect.contains_with_margin(fe.abs.x, fe.abs.y, &opt.margin) {
                    fe.handled = true;
                    return Event::FingerScroll(FingerScrollEvent {
                        rel: Vec2 {x: fe.abs.x - rect.x, y: fe.abs.y - rect.y},
                        rect: rect,
                        ..fe.clone()
                    })
                }
            },
            Event::FingerHover(fe) => {
                let rect = area.get_rect(&cx, opt.no_scrolling);
                
                if cx._finger_over_last_area == area {
                    let mut any_down = false;
                    for fin_area in &cx.captured_fingers {
                        if *fin_area == area {
                            any_down = true;
                            break;
                        }
                    }
                    if !fe.handled && rect.contains_with_margin(fe.abs.x, fe.abs.y, &opt.margin) {
                        fe.handled = true;
                        if let HoverState::Out = fe.hover_state {
                            //    cx.finger_over_last_area = Area::Empty;
                        }
                        else {
                            cx.finger_over_last_area = area;
                        }
                        return Event::FingerHover(FingerHoverEvent {
                            rel: area.abs_to_rel(cx, fe.abs, opt.no_scrolling),
                            rect: rect,
                            any_down:any_down,
                            ..fe.clone()
                        })
                    }
                    else {
                        //self.was_over_last_call = false;
                        return Event::FingerHover(FingerHoverEvent {
                            rel: area.abs_to_rel(cx, fe.abs, opt.no_scrolling),
                            rect: rect,
                            any_down:any_down,
                            hover_state: HoverState::Out,
                            ..fe.clone()
                        })
                    }
                }
                else {
                    if !fe.handled && rect.contains_with_margin(fe.abs.x, fe.abs.y, &opt.margin) {
                        let mut any_down = false;
                        for fin_area in &cx.captured_fingers {
                            if *fin_area == area {
                                any_down = true;
                                break;
                            }
                        }
                        cx.finger_over_last_area = area;
                        fe.handled = true;
                        //self.was_over_last_call = true;
                        return Event::FingerHover(FingerHoverEvent {
                            rel: area.abs_to_rel(cx, fe.abs, opt.no_scrolling),
                            rect: rect,
                            any_down:any_down,
                            hover_state: HoverState::In,
                            ..fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe) => {
                // check wether our digit is captured, otherwise don't send
                if cx.captured_fingers[fe.digit] == area {
                    let abs_start = cx.finger_down_abs_start[fe.digit];
                    let rel_start = cx.finger_down_rel_start[fe.digit];
                    let rect = area.get_rect(&cx, opt.no_scrolling);
                    return Event::FingerMove(FingerMoveEvent {
                        abs_start: abs_start,
                        rel: area.abs_to_rel(cx, fe.abs, opt.no_scrolling),
                        rel_start: rel_start,
                        rect: rect,
                        is_over: rect.contains_with_margin(fe.abs.x, fe.abs.y, &opt.margin),
                        ..fe.clone()
                    })
                }
            },
            Event::FingerDown(fe) => {
                if !fe.handled {
                    let rect = area.get_rect(&cx, opt.no_scrolling);
                    if rect.contains_with_margin(fe.abs.x, fe.abs.y, &opt.margin) {
                        // scan if any of the fingers already captured this area
                        if !opt.use_multi_touch {
                            for fin_area in &cx.captured_fingers {
                                if *fin_area == area {
                                    return Event::None;
                                }
                            }
                        }
                        cx.captured_fingers[fe.digit] = area;
                        let rel = area.abs_to_rel(cx, fe.abs, opt.no_scrolling);
                        cx.finger_down_abs_start[fe.digit] = fe.abs;
                        cx.finger_down_rel_start[fe.digit] = rel;
                        fe.handled = true;
                        return Event::FingerDown(FingerDownEvent {
                            rel: rel,
                            rect: rect,
                            ..fe.clone()
                        })
                    }
                }
            },
            Event::FingerUp(fe) => {
                if cx.captured_fingers[fe.digit] == area {
                    cx.captured_fingers[fe.digit] = Area::Empty;
                    let abs_start = cx.finger_down_abs_start[fe.digit];
                    let rel_start = cx.finger_down_rel_start[fe.digit];
                    let rect = area.get_rect(&cx, opt.no_scrolling);
                    return Event::FingerUp(FingerUpEvent {
                        is_over: rect.contains(fe.abs.x, fe.abs.y),
                        abs_start: abs_start,
                        rel_start: rel_start,
                        rel: area.abs_to_rel(cx, fe.abs, opt.no_scrolling),
                        rect: rect,
                        ..fe.clone()
                    })
                }
            },
            _ => ()
        };
        return Event::None;
    }
}

#[derive(PartialEq, Clone, Copy, Debug, Default)]
pub struct Signal {
    pub signal_id: usize
}

impl Signal {
    pub fn empty() -> Signal {
        Signal {
            signal_id: 0
        }
    }
    
    pub fn is_empty(&self) -> bool {
        self.signal_id == 0
    }
    
    pub fn is_signal(&self, se: &SignalEvent) -> bool {
        se.signal_id == self.signal_id
    }
}

#[derive(Clone, Debug, Default)]
pub struct FileRead {
    pub path: String,
    pub read_id: u64
}

impl FileRead {
    pub fn is_pending(&self) -> bool {
        self.read_id != 0
    }
    
    pub fn resolve_utf8<'a>(&mut self, fr: &'a FileReadEvent) -> Option<Result<&'a str,
    String>> {
        if fr.read_id == self.read_id {
            self.read_id = 0;
            if let Ok(str_data) = &fr.data {
                if let Ok(utf8_string) = std::str::from_utf8(&str_data) {
                    return Some(Ok(utf8_string))
                }
                else {
                    return Some(Err(format!("can't parse file as utf8 {}", self.path)))
                }
            }
            else if let Err(err) = &fr.data {
                return Some(Err(format!("can't load file as utf8 {} {}", self.path, err)))
            }
        }
        return None
    }
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
            Event::FingerScroll(fe) => {
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
            Event::FingerScroll(fe) => {
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
#[derive(Clone, PartialEq, Debug)]
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

