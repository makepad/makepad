use {
    std::{
        collections::HashMap
    },
    makepad_shader_compiler::makepad_live_compiler::LiveEditEvent,
    crate::{
        cx::Cx,
        area::Area,
        event::{
            finger::*,
            keyboard::*,
            window::*,
            xr::*,
        },
        cursor::MouseCursor,
        menu::CommandId,
    },
};

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    None,
    Construct,
    Destruct,
    Paint,
    Draw(DrawEvent),
    LiveEdit(LiveEditEvent),
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
    Trigger(TriggerEvent),
    Command(CommandId),
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy(TextCopyEvent),
    FingerDrag(FingerDragEvent),
    FingerDrop(FingerDropEvent),
    DragEnd,
}

pub enum HitEvent<'a>{
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    Trigger(TriggerHitEvent<'a>),
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

#[derive(Clone, Default, Debug, PartialEq)]
pub struct DrawEvent {
    pub redraw_views: Vec<usize>,
    pub redraw_views_and_children: Vec<usize>,
    pub redraw_all_views: bool,
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct NextFrameEvent {
    pub frame: u64,
    pub time: f64
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimerEvent {
    pub timer_id: u64
}

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

#[derive(Clone, Debug, PartialEq)]
pub struct SignalEvent {
    pub signals: HashMap<Signal, Vec<u64>>
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerEvent {
    pub triggers: HashMap<Area, Vec<u64>>
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerHitEvent<'a>(pub &'a [u64]);


impl Default for Event {
    fn default() -> Event {
        Event::None
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Copy, Hash)]
pub struct NextFrame(pub u64);

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
    
    pub fn is_next_frame(&self, cx: &mut Cx, next_frame: NextFrame) -> Option<NextFrameEvent> {
        match self {
            Event::NextFrame(fe) => {
                if cx.next_frames.contains(&next_frame) {
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
}
