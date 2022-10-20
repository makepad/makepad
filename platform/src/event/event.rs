use {
    std::{
        collections::{HashSet, HashMap}
    },
    crate::{
        makepad_live_compiler::LiveEditEvent,
        makepad_live_id::LiveId,
        cx::Cx,
        area::Area,
        //midi::{Midi1InputData, MidiInputInfo},
        event::{
            finger::*,
            keyboard::*,
            window::*,
            xr::*,
        },
        draw_list::DrawListId,
        menu::MenuCommand,
    },
};


#[derive(Clone, Debug)]
pub enum Event {
    Construct,
    Destruct,

    Draw(DrawEvent),
    LiveEdit(LiveEditEvent),
    AppGotFocus,
    AppLostFocus,
    NextFrame(NextFrameEvent),
    XRUpdate(XRUpdateEvent),
    
    //WindowSetHoverCursor(MouseCursor),
    WindowDragQuery(WindowDragQueryEvent),
    WindowCloseRequested(WindowCloseRequestedEvent),
    WindowClosed(WindowClosedEvent),
    WindowGeomChange(WindowGeomChangeEvent),
    
    FingerDown(FingerDownEvent),
    FingerMove(FingerMoveEvent),
    FingerHover(FingerHoverEvent),
    FingerUp(FingerUpEvent),
    FingerScroll(FingerScrollEvent),
    Timer(TimerEvent),
    
    Signal(SignalEvent),
    Trigger(TriggerEvent),
    MenuCommand(MenuCommand),
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy(TextCopyEvent),
    
    Drag(DragEvent),
    Drop(DropEvent),
    DragEnd,
    
    WebSocketClose(WebSocket),
    WebSocketOpen(WebSocket),
    WebSocketError(WebSocketErrorEvent),
    WebSocketMessage(WebSocketMessageEvent),
    
    #[cfg(target_arch = "wasm32")]
    ToWasmMsg(ToWasmMsgEvent),
    //Midi1InputData(Vec<Midi1InputData>),
    //MidiInputList(MidiInputListEvent),
}

pub enum Hit{
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    Trigger(TriggerHitEvent),
    TextInput(TextInputEvent),
    TextCopy(TextCopyEvent),
    FingerScroll(FingerScrollHitEvent),
    FingerDown(FingerDownHitEvent),
    FingerMove(FingerMoveHitEvent),
    FingerHoverIn(FingerHoverHitEvent),
    FingerHoverOver(FingerHoverHitEvent),
    FingerHoverOut(FingerHoverHitEvent),
    FingerUp(FingerUpHitEvent),
    
    FingerSweep(FingerSweepEvent),
    FingerSweepIn(FingerSweepEvent),
    FingerSweepOut(FingerSweepEvent),
    Nothing
}

pub enum DragHit<'a>{
    Drag(DragHitEvent<'a>),
    Drop(DropHitEvent<'a>),
    DragEnd,
    NoHit
}

#[derive(Clone, Debug)]
pub struct TriggerEvent {
    pub triggers: HashMap<Area, Vec<Trigger>>
}

/*
#[derive(Clone, Debug)]
pub struct MidiInputListEvent {
    pub inputs: Vec<MidiInputInfo>,
}*/

#[derive(Clone, Debug, Default)]
pub struct DrawEvent {
    pub draw_lists: Vec<DrawListId>,
    pub draw_lists_and_children: Vec<DrawListId>,
    pub redraw_all: bool,
}

impl DrawEvent{
    pub fn will_redraw(&self) -> bool {
        self.redraw_all
            || self.draw_lists.len() != 0
            || self.draw_lists_and_children.len() != 0
    }
    
    pub fn draw_list_will_redraw(&self, cx:&Cx, draw_list_id:DrawListId)->bool{
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

#[derive(Clone, Default, Debug)]
pub struct NextFrameEvent {
    pub frame: u64,
    pub time: f64,
    pub set: HashSet<NextFrame>
}

#[derive(Clone, Debug)]
pub struct TimerEvent {
    pub timer_id: u64
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct Signal(pub LiveId);
impl From<LiveId> for Signal {
    fn from(live_id: LiveId) -> Signal {Signal(live_id)}
}


#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct Trigger{
    pub id:LiveId,
    pub from:Area
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerHitEvent(pub Vec<Trigger>);

pub enum WebSocketAutoReconnect{
    Yes,
    No
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct WebSocket(pub u64);

#[derive(Clone, Debug)]
pub struct WebSocketErrorEvent {
    pub web_socket: WebSocket,
    pub error: String
}

#[derive(Clone, Debug)]
pub struct WebSocketMessageEvent {
    pub web_socket: WebSocket,
    pub data: Vec<u8>
}

#[derive(Clone, Debug)]
pub struct SignalEvent {
    pub signals: HashSet<Signal>
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Copy, Hash)]
pub struct NextFrame(pub u64);

impl NextFrame{
    pub fn is_event(&self, event:&Event)->Option<NextFrameEvent>{
        if let Event::NextFrame(ne) = event{
            if ne.set.contains(&self){
                return Some(ne.clone())
            }
        }
        None
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Timer(pub u64);

impl Timer {
    pub fn is_event(&self, event:&Event)->bool{
        if let Event::Timer(te) = event{
            if te.timer_id == self.0{
                return true
            }
        }
        false
    }
    
    pub fn empty() -> Timer {
        Timer(0)
    }
    
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

#[cfg(target_arch = "wasm32")]
use crate::makepad_wasm_bridge::ToWasmMsg;

#[cfg(target_arch = "wasm32")]
use crate::makepad_wasm_bridge::ToWasmMsgRef;

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug)]
pub struct ToWasmMsgEvent{
    pub id: LiveId,
    pub msg: ToWasmMsg,
    pub offset: usize
}

#[cfg(target_arch = "wasm32")]
impl ToWasmMsgEvent{
    pub fn as_ref(&self)->ToWasmMsgRef{self.msg.as_ref_at(self.offset)}
}
