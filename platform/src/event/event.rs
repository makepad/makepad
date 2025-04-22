use {
    std::{
        rc::Rc,
        cell::Cell,
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
    XrUpdate(XrUpdateEvent),

    WindowDragQuery(WindowDragQueryEvent),
    WindowCloseRequested(WindowCloseRequestedEvent),
    WindowClosed(WindowClosedEvent),
    WindowGeomChange(WindowGeomChangeEvent),
    VirtualKeyboard(VirtualKeyboardEvent),
    ClearAtlasses,

    /// The raw event that occurs when the user presses a mouse button down.
    ///
    /// Do not match upon or handle this event directly; instead, use the family of
    /// `hit`` functions ([`Event::hits()`]) and handle the returned [`Hit::FingerDown`].
    MouseDown(MouseDownEvent),
    /// The raw event that occurs when the user moves the mouse.
    ///
    /// Do not match upon or handle this event directly; instead, use the family of
    /// `hit` functions ([`Event::hits()`]) and handle the returned [`Hit`].
    MouseMove(MouseMoveEvent),
    /// The raw event that occurs when the user releases a previously-pressed mouse button.
    ///
    /// Do not match upon or handle this event directly; instead, use the family of
    /// `hit` functions ([`Event::hits()`]) and handle the returned [`Hit::FingerUp`].
    MouseUp(MouseUpEvent),
    /// The raw event that occurs when the user moves the mouse outside of the window.
    ///
    /// Do not match upon or handle this event directly; instead, use the family of
    /// `hit` functions ([`Event::hits()`]) and handle the returned [`Hit::FingerOverOut`].
    MouseLeave(MouseLeaveEvent),
    /// The raw event that occurs when the user touches the screen.
    ///
    /// Do not match upon or handle this event directly; instead, use the family of
    /// `hit` functions ([`Event::hits()`]) and handle the returned [`Hit`].
    TouchUpdate(TouchUpdateEvent),
    /// The raw event that occurs when the user finishes a long press touch/click.
    ///
    /// Do not match upon or handle this event directly; instead, use the family of
    /// `hit` functions ([`Event::hits()`]) and handle the returned [`Hit::FingerLongPress`].
    LongPress(LongPressEvent),
    /// The raw event that occurs when the user scrolls, e.g.,
    /// by using the mouse wheel or a touch flick.
    ///
    /// Do not match upon or handle this event directly; instead use the family of
    /// `hit` functions ([`Event::hits()`]) and handle the returned [`Hit::FingerScroll`].
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
    
    /// The "go back" navigational button or gesture was performed.
    ///
    /// Tip: use the [`Event::consume_back_pressed()`] method to handle this event
    /// instead of matching on it directly.
    ///
    /// Once a widget has handled this event, it should set the `handled` flag to `true`
    /// to ensure that a single "go back" action is not handled multiple times.
    BackPressed {
        handled: Cell<bool>,
    },
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
            23=>"LongPress",
            24=>"Scroll",

            25=>"Timer",

            26=>"Signal",
            27=>"Trigger",
            28=>"MacosMenuCommand",
            29=>"KeyFocus",
            30=>"KeyFocusLost",
            31=>"KeyDown",
            32=>"KeyUp",
            33=>"TextInput",
            34=>"TextCopy",
            35=>"TextCut",

            36=>"Drag",
            37=>"Drop",
            38=>"DragEnd",

            39=>"AudioDevices",
            40=>"MidiPorts",
            41=>"VideoInputs",
            42=>"NetworkResponses",

            43=>"VideoPlaybackPrepared",
            44=>"VideoTextureUpdated",
            45=>"VideoPlaybackCompleted",
            46=>"VideoDecodingError",
            47=>"VideoPlaybackResourcesReleased",
            48=>"TextureHandleReady",
            49=>"MouseLeave",
            50=>"Actions",
            51=>"BackPressed",

            #[cfg(target_arch = "wasm32")]
            52=>"ToWasmMsg",
            
            53=>"DesignerPick",
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
            Self::XrUpdate(_)=>12,

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
            Self::LongPress(_)=>23,
            Self::Scroll(_)=>24,

            Self::Timer(_)=>25,

            Self::Signal=>26,
            Self::Trigger(_)=>27,
            Self::MacosMenuCommand(_)=>28,
            Self::KeyFocus(_)=>29,
            Self::KeyFocusLost(_)=>30,
            Self::KeyDown(_)=>31,
            Self::KeyUp(_)=>32,
            Self::TextInput(_)=>33,
            Self::TextCopy(_)=>34,
            Self::TextCut(_)=>35,

            Self::Drag(_)=>36,
            Self::Drop(_)=>37,
            Self::DragEnd=>38,

            Self::AudioDevices(_)=>39,
            Self::MidiPorts(_)=>40,
            Self::VideoInputs(_)=>41,
            Self::NetworkResponses(_)=>42,

            Self::VideoPlaybackPrepared(_)=>43,
            Self::VideoTextureUpdated(_)=>44,
            Self::VideoPlaybackCompleted(_)=>45,
            Self::VideoDecodingError(_)=>46,
            Self::VideoPlaybackResourcesReleased(_)=>47,
            Self::TextureHandleReady(_)=>48,
            Self::MouseLeave(_)=>49,
            Self::Actions(_)=>50,
            Self::BackPressed{..}=>51,
            
            #[cfg(target_arch = "wasm32")]
            Self::ToWasmMsg(_)=>52,
            
            Self::DesignerPick(_) =>53,
        }
    }

    /// A convenience function to check if the event is a [`BackPressed`] event
    /// that has not yet been handled, and then mark it as handled.
    ///
    /// Returns `true` if the event was a [`BackPressed`] event that wasn't already handled.
    pub fn back_pressed(&self) -> bool {
        if let Self::BackPressed { handled } = self {
            if !handled.get() {
                handled.set(true);
                return true;
            }
        }
        false
    }
}


#[derive(Debug)]
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
    FingerLongPress(FingerLongPressEvent),
    
    DesignerPick(DesignerPickEvent),

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
    pub xr_state: Option<Rc<XrState>>
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
                if let Some(n) = cx.draw_lists.checked_index(vw){
                    next = n.codeflow_parent_id;
                }
                else{ // a drawlist in our redraw lists was reused
                    break;
                }
            }
        }
        // figure out if areas are in some way a parent of view_id, then redraw
        for check_draw_list_id in &self.draw_lists_and_children {
            let mut next = Some(draw_list_id);
            while let Some(vw) = next{
                if vw == *check_draw_list_id {
                    return true
                }
                if let Some(n) = cx.draw_lists.checked_index(vw){
                    next = n.codeflow_parent_id;
                }
                else{ // a drawlist in our redraw lists was reused
                    break;
                }
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
