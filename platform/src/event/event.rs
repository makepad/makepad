use {
    std::{
        collections::{HashSet, HashMap}
    },
    crate::{
        //makepad_live_compiler::LiveEditEvent,
        makepad_live_id::LiveId,
        cx::Cx,
        area::Area,
        //midi::{Midi1InputData, MidiInputInfo},
        event::{
            finger::*,
            keyboard::*,
            window::*,
            xr::*,
            drag_drop::*,
            network::*,
            video_decoding::*,
        },
        animator::Ease,
        audio::AudioDevicesEvent,
        midi::MidiPortsEvent,
        video::VideoInputsEvent,
        draw_list::DrawListId,
    },
};


#[derive(Clone, Debug)]
pub enum Event {
    Construct,
    Destruct,
    
    Pause,
    Resume,

    Draw(DrawEvent),
    LiveEdit,
    AppGotFocus,
    AppLostFocus,
    NextFrame(NextFrameEvent),
    XRUpdate(XRUpdateEvent),
    
    WindowDragQuery(WindowDragQueryEvent),
    WindowCloseRequested(WindowCloseRequestedEvent),
    WindowClosed(WindowClosedEvent),
    WindowGeomChange(WindowGeomChangeEvent),
    VirtualKeyboard(VirtualKeyboardEvent),
    ClearAtlasses,
    
    MouseDown(MouseDownEvent),
    MouseMove(MouseMoveEvent),
    MouseUp(MouseUpEvent),
    MouseLeave(MouseLeaveEvent),
    TouchUpdate(TouchUpdateEvent),
    Scroll(ScrollEvent), // this is the MouseWheel / touch scroll event sent by the OS
    
    Timer(TimerEvent),
    
    Signal,
    Trigger(TriggerEvent),
    MacosMenuCommand(LiveId),
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy(TextClipboardEvent),
    TextCut(TextClipboardEvent),
    
    Drag(DragEvent),
    Drop(DropEvent),
    DragEnd,
    
    AudioDevices(AudioDevicesEvent),
    MidiPorts(MidiPortsEvent),
    VideoInputs(VideoInputsEvent),
    NetworkResponses(Vec<NetworkResponseEvent>),

    VideoStream(VideoStreamEvent),
    VideoDecodingInitialized(VideoDecodingInitializedEvent),
    VideoChunkDecoded(LiveId),
    VideoDecodingError(VideoDecodingErrorEvent),
 
    #[cfg(target_arch = "wasm32")]
    ToWasmMsg(ToWasmMsgEvent),
}

impl Event{
    pub fn name_from_u32(v:u32)->&'static str{
        match v{
            1=>"Construct",
            2=>"Destruct",
                
            3=>"Pause",
            4=>"Resume",
            
            5=>"Draw",
            6=>"LiveEdit",
            7=>"AppGotFocus",
            8=>"AppLostFocus",
            9=>"NextFrame",
            10=>"XRUpdate",
                
            11=>"WindowDragQuery",
            12=>"WindowCloseRequested",
            13=>"WindowClosed",
            14=>"WindowGeomChange",
            15=>"VirtualKeyboard",
            16=>"ClearAtlasses",
                
            17=>"MouseDown",
            18=>"MouseMove",
            19=>"MouseUp",
            20=>"TouchUpdate",
            21=>"Scroll",
                
            22=>"Timer",
                
            23=>"Signal",
            24=>"Trigger",
            25=>"MacosMenuCommand",
            26=>"KeyFocus",
            27=>"KeyFocusLost",
            28=>"KeyDown",
            29=>"KeyUp",
            30=>"TextInput",
            31=>"TextCopy",
            32=>"TextCut",
                
            33=>"Drag",
            34=>"Drop",
            35=>"DragEnd",
                
            36=>"AudioDevices",
            37=>"MidiPorts",
            38=>"VideoInputs",
            39=>"NetworkResponses",
            
            40=>"VideoStream",
            41=>"VideoDecodingInitialized",
            42=>"VideoChunkDecoded",
            43=>"VideoDecodingError",
             
            #[cfg(target_arch = "wasm32")]
            44=>"ToWasmMsg",
            _=>panic!()
        }
    }
    
    pub fn to_u32(&self)->u32{
        match self{
            Self::Construct=>1,
            Self::Destruct=>2,
                            
            Self::Pause=>3,
            Self::Resume=>4,
                        
            Self::Draw(_)=>5,
            Self::LiveEdit=>6,
            Self::AppGotFocus=>7,
            Self::AppLostFocus=>8,
            Self::NextFrame(_)=>9,
            Self::XRUpdate(_)=>10,
                            
            Self::WindowDragQuery(_)=>11,
            Self::WindowCloseRequested(_)=>12,
            Self::WindowClosed(_)=>13,
            Self::WindowGeomChange(_)=>14,
            Self::VirtualKeyboard(_)=>15,
            Self::ClearAtlasses=>16,
                            
            Self::MouseDown(_)=>17,
            Self::MouseMove(_)=>18,
            Self::MouseUp(_)=>19,
            Self::TouchUpdate(_)=>20,
            Self::Scroll(_)=>21,
                            
            Self::Timer(_)=>22,
                            
            Self::Signal=>23,
            Self::Trigger(_)=>24,
            Self::MacosMenuCommand(_)=>25,
            Self::KeyFocus(_)=>26,
            Self::KeyFocusLost(_)=>27,
            Self::KeyDown(_)=>28,
            Self::KeyUp(_)=>29,
            Self::TextInput(_)=>30,
            Self::TextCopy(_)=>31,
            Self::TextCut(_)=>32,
                            
            Self::Drag(_)=>33,
            Self::Drop(_)=>34,
            Self::DragEnd=>35,
                            
            Self::AudioDevices(_)=>36,
            Self::MidiPorts(_)=>37,
            Self::VideoInputs(_)=>38,
            Self::NetworkResponses(_)=>39,
                        
            Self::VideoStream(_)=>40,
            Self::VideoDecodingInitialized(_)=>41,
            Self::VideoChunkDecoded(_)=>42,
            Self::VideoDecodingError(_)=>43,
                         
            #[cfg(target_arch = "wasm32")]
            Self::ToWasmMsg(_)=>44,
        }
    }
}


pub enum Hit{
    KeyFocus(KeyFocusEvent),
    KeyFocusLost(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    Trigger(TriggerHitEvent),
    TextInput(TextInputEvent),
    TextCopy(TextClipboardEvent),
    TextCut(TextClipboardEvent),
    
    FingerScroll(FingerScrollEvent),
    FingerDown(FingerDownEvent),
    FingerMove(FingerMoveEvent),
    FingerHoverIn(FingerHoverEvent),
    FingerHoverOver(FingerHoverEvent),
    FingerHoverOut(FingerHoverEvent),
    FingerUp(FingerUpEvent),
    
    Nothing
}

#[derive(Clone)]
pub enum DragHit{
    Drag(DragHitEvent),
    Drop(DropHitEvent),
    DragEnd,
    NoHit
}

impl Event{
    pub fn requires_visibility(&self)->bool{
        match self{
            Self::MouseDown(_)|
            Self::MouseMove(_)|
            Self::MouseUp(_)|
            Self::TouchUpdate(_)|
            Self::Scroll(_)=>true,
            _=>false
        }
    }
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


#[derive(Clone, Debug)]
pub enum VirtualKeyboardEvent{
    WillShow{time:f64, height:f64, duration:f64, ease:Ease},
    WillHide{time:f64, height:f64, duration:f64, ease:Ease},
    DidShow{time:f64, height:f64},
    DidHide{time:f64},
}

#[derive(Clone, Default, Debug)]
pub struct NextFrameEvent {
    pub frame: u64,
    pub time: f64,
    pub set: HashSet<NextFrame>
}

#[derive(Clone, Debug)]
pub struct TimerEvent {
    pub time: Option<f64>,
    pub timer_id: u64
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct Trigger{
    pub id:LiveId,
    pub from:Area
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerHitEvent(pub Vec<Trigger>);

#[derive(Clone, Debug)]
pub struct WebSocketErrorEvent {
    pub socket_id: LiveId,
    pub error: String
}

#[derive(Clone, Debug)]
pub struct WebSocketMessageEvent {
    pub socket_id: LiveId,
    pub data: Vec<u8>
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
    pub fn is_event(&self, event:&Event)->Option<TimerEvent>{
        if let Event::Timer(te) = event{
            if te.timer_id == self.0{
                return Some(te.clone())
            }
        }
        None
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
