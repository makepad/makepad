use {
    std::{
        collections::{HashSet, HashMap}
    },
    crate::{
        makepad_live_compiler::LiveEditEvent,
        makepad_live_id::LiveId,
        cx::Cx,
        area::Area,
        event::{
            finger::*,
            keyboard::*,
            window::*,
            xr::*,
        },
        cursor::MouseCursor,
        menu::Command,
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
    AppGotFocus,
    AppLostFocus,
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
    Command(Command),
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy(TextCopyEvent),
    FingerDrag(FingerDragEvent),
    FingerDrop(FingerDropEvent),
    DragEnd,
    
    WebSocketClose(WebSocket),
    WebSocketOpen(WebSocket),
    WebSocketError(WebSocketErrorEvent),
    WebSocketMessage(WebSocketMessageEvent),
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

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerEvent {
    pub triggers: HashMap<Area, HashSet<Trigger>>
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct DrawEvent {
    pub draw_lists: Vec<usize>,
    pub draw_lists_and_children: Vec<usize>,
    pub redraw_all: bool,
}

impl DrawEvent{
    pub fn will_redraw(&self) -> bool {
        self.redraw_all
            || self.draw_lists.len() != 0
            || self.draw_lists_and_children.len() != 0
    }
    
    pub fn draw_list_will_redraw(&self, cx:&Cx, draw_list_id:usize)->bool{
         if self.redraw_all {
            return true;
        }
        // figure out if areas are in some way a child of view_id, then we need to redraw
        for check_draw_list_id in &self.draw_lists {
            let mut next = Some(*check_draw_list_id);
            while let Some(vw) = next{
                if vw == draw_list_id {
                    return true
                }
                next = cx.draw_lists[vw].codeflow_parent_id;
            }
        }
        // figure out if areas are in some way a parent of view_id, then redraw
        for check_draw_list_id in &self.draw_lists_and_children {
            let mut next = Some(draw_list_id);
            while let Some(vw) = next{
                if vw == *check_draw_list_id {
                    return true
                }
                next = cx.draw_lists[vw].codeflow_parent_id;
            }
        }
        false
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct NextFrameEvent {
    pub frame: u64,
    pub time: f64,
    pub set: HashSet<NextFrame>
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimerEvent {
    pub timer_id: u64
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct Signal(pub LiveId);
impl From<LiveId> for Signal {
    fn from(live_id: LiveId) -> Signal {Signal(live_id)}
}


#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct Trigger(pub LiveId);
impl From<LiveId> for Trigger {
    fn from(live_id: LiveId) -> Trigger {Trigger(live_id)}
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerHitEvent<'a>(pub &'a HashSet<Trigger>);


pub enum WebSocketReconnect{
    Automatic,
    Manual
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct WebSocket(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub struct WebSocketErrorEvent {
    pub web_socket: WebSocket,
    pub error: String
}

#[derive(Clone, Debug, PartialEq)]
pub struct WebSocketMessageEvent {
    pub web_socket: WebSocket,
    pub data: Vec<u8>
}

#[derive(Clone, Debug, PartialEq)]
pub struct SignalEvent {
    pub signals: HashSet<Signal>
}

impl Default for Event {
    fn default() -> Event {
        Event::None
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Copy, Hash)]
pub struct NextFrame(pub u64);

#[derive(Copy, Clone, Debug, Default)]
pub struct Timer(pub u64);

impl Timer {
    pub fn empty() -> Timer {
        Timer(0)
    }
    
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    
    pub fn is_timer(&mut self, te: &TimerEvent) -> bool {
        te.timer_id == self.0
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
    
    pub fn is_next_frame<'a>(&'a self, next_frame: NextFrame) -> Option<&'a NextFrameEvent> {
        match self {
            Event::NextFrame(fe) => {
                if fe.set.contains(&next_frame) {
                    return Some(&fe)
                }
            }
            _ => ()
        }
        None
    }
    
    pub fn is_timer(&self, timer: Timer) -> bool{
        match self {
            Event::Timer(te) => {
                return te.timer_id == timer.0
            }
            _ => ()
        }
        false
    }    
}
