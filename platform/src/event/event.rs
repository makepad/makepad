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
            designer::*,
            network::*,
            video_playback::*,
        },
        action::ActionsBuf,
        animator::Ease,
        audio::AudioDevicesEvent,
        midi::MidiPortsEvent,
        video::VideoInputsEvent,
        draw_list::DrawListId,
    },
};

/// Events that can be sent between the Makepad framework and the application.
#[derive(Debug)]
pub enum Event {
    /// The application has just been created.
    ///
    /// * This event is always sent exactly once (before any other event)
    ///   at the very beginning of the application lifecycle.
    /// * This is a good point for one-time initialization of application state, resources, tasks, etc.
    ///
    /// | Platform | Lifecycle Function/Callback |
    /// |----------|-----------------------------|
    /// | Android  | [`onCreate`]                |
    /// | others   | coming soon...              |
    ///
    /// [`onCreate`]: https://developer.android.com/reference/android/app/Activity#onCreate(android.os.Bundle)
    Startup,
    /// The application is being shut down is about to close and be destroyed.
    ///
    /// * This event may not be sent at all, so you should not rely on it.
    ///   * For example, some mobile platforms do not always send this event when closing the app.
    ///   * Desktop platforms do typically send this event when the user closes the application.
    /// * If it is sent, it will be sent only once at the end of the application lifecycle.
    ///
    /// | Platform | Lifecycle Function/Callback |
    /// |----------|-----------------------------|
    /// | Android  | [`onDestroy`]               |
    /// | others   | coming soon...              |
    ///
    /// [`onDestroy`]: https://developer.android.com/reference/android/app/Activity#onDestroy()
    Shutdown,

    /// The application has been started in the foreground and is now visible to the user,
    /// but is not yet actively receiving user input.
    ///
    /// * This event can be sent multiple times over the course of the application's lifecycle.
    ///   * For example, it will be sent right after `Startup` has been sent
    ///     at the beginning of the application.
    ///   * It can also be sent after `Stop` if the user starts the application again
    ///     by navigating back to the application.
    ///   * It will be sent when the application was re-opened and shown again
    ///     after being previously hidden in the background.
    ///
    /// | Platform | Lifecycle Function/Callback |
    /// |----------|-----------------------------|
    /// | Android  | [`onStart`]                 |
    /// | others   | coming soon...              |
    ///
    /// [`onStart`]: https://developer.android.com/reference/android/app/Activity#onStart(
    #[doc(alias("start, restart, show"))]
    Foreground,
    /// The application has been hidden in the background and is no longer visible to the user.
    ///
    /// * This event may be sent multiple times over the course of the application's lifecycle.
    ///   * For example, it can be sent after `Pause` has been sent, i.e., when the user
    ///     navigates away from the application, causing it to be no longer visible.
    /// * This is a good point to stop updating the UI/animations and other visual elements.
    ///
    /// | Platform | Lifecycle Function/Callback |
    /// |----------|-----------------------------|
    /// | Android  | [`onStop`]                  |
    /// | others   | coming soon...              |
    ///
    /// [`onStop`]: https://developer.android.com/reference/android/app/Activity#onStop()
    #[doc(alias("stop, hide")) ]
    Background,

    /// The application is now in the foreground and being actively used,
    /// i.e., it is receiving input from the user.
    ///
    /// * This event may be sent multiple times over the course of the application's lifecycle.
    ///   * For example, it will be sent after `Start` once the application is fully in the foreground.
    ///     It can also be sent after `Pause`, once the user navigates back to the application.
    ///
    /// | Platform | Lifecycle Function/Callback |
    /// |----------|-----------------------------|
    /// | Android  | [`onResume`]                |
    /// | others   | coming soon...              |
    ///
    /// [`onResume`]: https://developer.android.com/reference/android/app/Activity#onResume()
    Resume,
    /// The application has been temporarily paused and is still visible in the foregound,
    /// but is not actively receiving input from the user.
    ///
    /// * This event may be sent multiple times over the course of the application's lifecycle.
    /// * This is a good point to save temporary application states in case the application
    ///   is about to be stopped or destroyed.
    ///
    /// | Platform | Lifecycle Function/Callback |
    /// |----------|-----------------------------|
    /// | Android  | [`onPause`]                 |
    /// | others   | coming soon...              |
    ///
    /// [`onPause`]: https://developer.android.com/reference/android/app/Activity#onPause()
    Pause,
    
    Draw(DrawEvent),
    LiveEdit,
    /// The application has gained focus and is now the active window receiving user input.
    AppGotFocus,
    /// The application has lost focus and is no longer the active window receiving user input.
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

    Actions(ActionsBuf),
    AudioDevices(AudioDevicesEvent),
    MidiPorts(MidiPortsEvent),
    VideoInputs(VideoInputsEvent),
    NetworkResponses(NetworkResponsesEvent),

    VideoPlaybackPrepared(VideoPlaybackPreparedEvent),
    VideoTextureUpdated(VideoTextureUpdatedEvent),
    VideoPlaybackCompleted(VideoPlaybackCompletedEvent),
    VideoPlaybackResourcesReleased(VideoPlaybackResourcesReleasedEvent),
    VideoDecodingError(VideoDecodingErrorEvent),
    TextureHandleReady(TextureHandleReadyEvent),
    
    BackPressed,
    #[cfg(target_arch = "wasm32")]
    ToWasmMsg(ToWasmMsgEvent),
    
    DesignerPick(DesignerPickEvent),
}

impl Event{
    pub fn name(&self)->&'static str{
        Self::name_from_u32(self.to_u32())
    }

    pub fn name_from_u32(v:u32)->&'static str{
        match v{
            1=>"Startup",
            2=>"Shutdown",

            3=>"Foreground",
            4=>"Background",

            5=>"Resume",
            6=>"Pause",

            7=>"Draw",
            8=>"LiveEdit",
            9=>"AppGotFocus",
            10=>"AppLostFocus",
            11=>"NextFrame",
            12=>"XRUpdate",

            13=>"WindowDragQuery",
            14=>"WindowCloseRequested",
            15=>"WindowClosed",
            16=>"WindowGeomChange",
            17=>"VirtualKeyboard",
            18=>"ClearAtlasses",

            19=>"MouseDown",
            20=>"MouseMove",
            21=>"MouseUp",
            22=>"TouchUpdate",
            23=>"Scroll",

            24=>"Timer",

            25=>"Signal",
            26=>"Trigger",
            27=>"MacosMenuCommand",
            28=>"KeyFocus",
            29=>"KeyFocusLost",
            30=>"KeyDown",
            31=>"KeyUp",
            32=>"TextInput",
            33=>"TextCopy",
            34=>"TextCut",

            35=>"Drag",
            36=>"Drop",
            37=>"DragEnd",

            38=>"AudioDevices",
            39=>"MidiPorts",
            40=>"VideoInputs",
            41=>"NetworkResponses",

            42=>"VideoPlaybackPrepared",
            43=>"VideoTextureUpdated",
            44=>"VideoPlaybackCompleted",
            45=>"VideoDecodingError",
            46=>"VideoPlaybackResourcesReleased",
            47=>"TextureHandleReady",
            48=>"MouseLeave",
            49=>"Actions",
            50=>"BackPressed",

            #[cfg(target_arch = "wasm32")]
            51=>"ToWasmMsg",
            
            52=>"DesignerPick",            
            _=>panic!()
        }
    }

    pub fn to_u32(&self)->u32{
        match self{
            Self::Startup=>1,
            Self::Shutdown=>2,

            Self::Foreground=>3,
            Self::Background=>4,

            Self::Resume=>5,
            Self::Pause=>6,

            Self::Draw(_)=>7,
            Self::LiveEdit=>8,
            Self::AppGotFocus=>9,
            Self::AppLostFocus=>10,
            Self::NextFrame(_)=>11,
            Self::XRUpdate(_)=>12,

            Self::WindowDragQuery(_)=>13,
            Self::WindowCloseRequested(_)=>14,
            Self::WindowClosed(_)=>15,
            Self::WindowGeomChange(_)=>16,
            Self::VirtualKeyboard(_)=>17,
            Self::ClearAtlasses=>18,

            Self::MouseDown(_)=>19,
            Self::MouseMove(_)=>20,
            Self::MouseUp(_)=>21,
            Self::TouchUpdate(_)=>22,
            Self::Scroll(_)=>23,

            Self::Timer(_)=>24,

            Self::Signal=>25,
            Self::Trigger(_)=>26,
            Self::MacosMenuCommand(_)=>27,
            Self::KeyFocus(_)=>28,
            Self::KeyFocusLost(_)=>29,
            Self::KeyDown(_)=>30,
            Self::KeyUp(_)=>31,
            Self::TextInput(_)=>32,
            Self::TextCopy(_)=>33,
            Self::TextCut(_)=>34,

            Self::Drag(_)=>35,
            Self::Drop(_)=>36,
            Self::DragEnd=>37,

            Self::AudioDevices(_)=>38,
            Self::MidiPorts(_)=>39,
            Self::VideoInputs(_)=>40,
            Self::NetworkResponses(_)=>41,

            Self::VideoPlaybackPrepared(_)=>42,
            Self::VideoTextureUpdated(_)=>43,
            Self::VideoPlaybackCompleted(_)=>44,
            Self::VideoDecodingError(_)=>45,
            Self::VideoPlaybackResourcesReleased(_)=>46,
            Self::TextureHandleReady(_)=>47,
            Self::MouseLeave(_)=>48,
            Self::Actions(_)=>49,
            Self::BackPressed=>50,
            
            #[cfg(target_arch = "wasm32")]
            Self::ToWasmMsg(_)=>51,
            
            Self::DesignerPick(_) =>52,
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
    
    DesignerPick(DesignerPickEvent),

    BackPressed,

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
    pub fn requires_visibility(&self) -> bool{
        match self{
            Self::MouseDown(_)|
            Self::MouseMove(_)|
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

    pub fn is_timer(&self, event:&TimerEvent)->Option<TimerEvent>{
        if event.timer_id == self.0{
            return Some(event.clone())
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
