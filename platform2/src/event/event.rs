use {
    std::{
        f64::consts::PI,
        rc::Rc,
        cell::Cell,
        collections::{HashSet, HashMap}
    },
    crate::{
        //makepad_live_compiler::LiveEditEvent,
        makepad_live_id::LiveId,
        makepad_script::*,
        //makepad_math::*,
        cx::Cx,
        area::Area,
        window::WindowId,
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
            game_input::*,
        },
        action::ActionsBuf,
        audio::AudioDevicesEvent,
        midi::MidiPortsEvent,
        video::VideoInputsEvent,
        draw_list::DrawListId,
        permission::{PermissionResult},
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
    /// A window has gained focus and is now the active window receiving user input.
    WindowGotFocus(WindowId),
    /// A window has lost focus and is no longer the active window receiving user input.
    WindowLostFocus(WindowId),
    GameInputConnected(GameInputConnectedEvent),
    NextFrame(NextFrameEvent),
    XrUpdate(XrUpdateEvent),
    XrLocal(XrLocalEvent),
    
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
    
    /// Permission check or request result
    PermissionResult(PermissionResult),
    
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
            9=>"WindowGotFocus",
            10=>"WindowLostFocus",
            11=>"GameInputConnected",
            12=>"NextFrame",
            13=>"XRUpdate",

            14=>"WindowDragQuery",
            15=>"WindowCloseRequested",
            16=>"WindowClosed",
            17=>"WindowGeomChange",
            18=>"VirtualKeyboard",
            19=>"ClearAtlasses",

            20=>"MouseDown",
            21=>"MouseMove",
            22=>"MouseUp",
            23=>"TouchUpdate",
            24=>"LongPress",
            25=>"Scroll",

            26=>"Timer",

            27=>"Signal",
            28=>"Trigger",
            29=>"MacosMenuCommand",
            30=>"KeyFocus",
            31=>"KeyFocusLost",
            32=>"KeyDown",
            33=>"KeyUp",
            34=>"TextInput",
            35=>"TextCopy",
            36=>"TextCut",

            37=>"Drag",
            38=>"Drop",
            39=>"DragEnd",

            40=>"AudioDevices",
            41=>"MidiPorts",
            42=>"VideoInputs",
            43=>"NetworkResponses",

            44=>"VideoPlaybackPrepared",
            45=>"VideoTextureUpdated",
            46=>"VideoPlaybackCompleted",
            47=>"VideoDecodingError",
            48=>"VideoPlaybackResourcesReleased",
            49=>"TextureHandleReady",
            50=>"MouseLeave",
            51=>"Actions",
            52=>"BackPressed",
            53=>"PermissionResult",

            #[cfg(target_arch = "wasm32")]
            54=>"ToWasmMsg",
            
            55=>"DesignerPick",
            56=>"XrLocal",
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
            Self::WindowGotFocus(_)=>9,
            Self::WindowLostFocus(_)=>10,
            Self::GameInputConnected(_)=>11,
            Self::NextFrame(_)=>12,
            Self::XrUpdate(_)=>13,

            Self::WindowDragQuery(_)=>14,
            Self::WindowCloseRequested(_)=>15,
            Self::WindowClosed(_)=>16,
            Self::WindowGeomChange(_)=>17,
            Self::VirtualKeyboard(_)=>18,
            Self::ClearAtlasses=>19,

            Self::MouseDown(_)=>20,
            Self::MouseMove(_)=>21,
            Self::MouseUp(_)=>22,
            Self::TouchUpdate(_)=>23,
            Self::LongPress(_)=>24,
            Self::Scroll(_)=>25,

            Self::Timer(_)=>26,

            Self::Signal=>27,
            Self::Trigger(_)=>28,
            Self::MacosMenuCommand(_)=>29,
            Self::KeyFocus(_)=>30,
            Self::KeyFocusLost(_)=>31,
            Self::KeyDown(_)=>32,
            Self::KeyUp(_)=>33,
            Self::TextInput(_)=>34,
            Self::TextCopy(_)=>35,
            Self::TextCut(_)=>36,

            Self::Drag(_)=>37,
            Self::Drop(_)=>38,
            Self::DragEnd=>39,

            Self::AudioDevices(_)=>40,
            Self::MidiPorts(_)=>41,
            Self::VideoInputs(_)=>42,
            Self::NetworkResponses(_)=>43,

            Self::VideoPlaybackPrepared(_)=>44,
            Self::VideoTextureUpdated(_)=>45,
            Self::VideoPlaybackCompleted(_)=>46,
            Self::VideoDecodingError(_)=>47,
            Self::VideoPlaybackResourcesReleased(_)=>48,
            Self::TextureHandleReady(_)=>49,
            Self::MouseLeave(_)=>50,
            Self::Actions(_)=>51,
            Self::BackPressed{..}=>52,
            Self::PermissionResult(_)=>53,
            
            #[cfg(target_arch = "wasm32")]
            Self::ToWasmMsg(_)=>54,
            
            Self::DesignerPick(_) =>55,
            Self::XrLocal(_)=>56,
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


#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
pub enum Ease {
    #[pick] Linear,
    #[live] None,
    #[live(1.0)] Constant(f64),
    #[live] InQuad,
    #[live] OutQuad,
    #[live] InOutQuad,
    #[live] InCubic,
    #[live] OutCubic,
    #[live] InOutCubic,
    #[live] InQuart,
    #[live] OutQuart,
    #[live] InOutQuart,
    #[live] InQuint,
    #[live] OutQuint,
    #[live] InOutQuint,
    #[live] InSine,
    #[live] OutSine,
    #[live] InOutSine,
    #[live] InExp,
    #[live] OutExp,
    #[live] InOutExp,
    #[live] InCirc,
    #[live] OutCirc,
    #[live] InOutCirc,
    #[live] InElastic,
    #[live] OutElastic,
    #[live] InOutElastic,
    #[live] InBack,
    #[live] OutBack,
    #[live] InOutBack,
    #[live] InBounce,
    #[live] OutBounce,
    #[live] InOutBounce,
    #[live {d1: 0.82, d2: 0.97, max: 100}] ExpDecay {d1: f64, d2: f64, max: usize},
        
    #[live {begin: 0.0, end: 1.0}] Pow {begin: f64, end: f64},
    #[live {cp0: 0.0, cp1: 0.0, cp2: 1.0, cp3: 1.0}] Bezier {cp0: f64, cp1: f64, cp2: f64, cp3: f64}
}

impl Ease {
    pub fn map(&self, t: f64) -> f64 {
        match self {
            Self::ExpDecay {d1, d2, max} => { // there must be a closed form for this
                if t > 0.999 {
                    return 1.0;
                }
                
                // first we count the number of steps we'd need to decay
                let mut di = *d1;
                let mut dt = 1.0;
                let max_steps = (*max).min(1000);
                let mut steps = 0;
                // for most of the settings we use this takes max 15 steps or so
                while dt > 0.001 && steps < max_steps {
                    steps = steps + 1;
                    dt = dt * di;
                    di *= d2;
                }
                // then we know how to find the step, and lerp it
                let step = t * (steps as f64);
                let mut di = *d1;
                let mut dt = 1.0;
                let max_steps = max_steps as f64;
                let mut steps = 0.0;
                while dt > 0.001 && steps < max_steps {
                    steps += 1.0;
                    if steps >= step { // right step
                        let fac = steps - step;
                        return 1.0 - (dt * fac + (dt * di) * (1.0 - fac))
                    }
                    dt = dt * di;
                    di *= d2;
                }
                1.0
            }
            Self::Linear => {
                return t.max(0.0).min(1.0);
            },
            Self::Constant(t) => {
                return t.max(0.0).min(1.0);
            },
            Self::None => {
                return 1.0;
            },
            Self::Pow {begin, end} => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                let a = -1. / (begin * begin).max(1.0);
                let b = 1. + 1. / (end * end).max(1.0);
                let t2 = (((a - 1.) * -b) / (a * (1. - b))).powf(t);
                return (-a * b + b * a * t2) / (a * t2 - b);
            },
                        
            Self::InQuad => {
                return t * t;
            },
            Self::OutQuad => {
                return t * (2.0 - t);
            },
            Self::InOutQuad => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t;
                }
                else {
                    let t = t - 1.;
                    return -0.5 * (t * (t - 2.) - 1.);
                }
            },
            Self::InCubic => {
                return t * t * t;
            },
            Self::OutCubic => {
                let t2 = t - 1.0;
                return t2 * t2 * t2 + 1.0;
            },
            Self::InOutCubic => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return 1. / 2. * (t * t * t + 2.);
                }
            },
            Self::InQuart => {
                return t * t * t * t
            },
            Self::OutQuart => {
                let t = t - 1.;
                return -(t * t * t * t - 1.);
            },
            Self::InOutQuart => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return -0.5 * (t * t * t * t - 2.);
                }
            },
            Self::InQuint => {
                return t * t * t * t * t;
            },
            Self::OutQuint => {
                let t = t - 1.;
                return t * t * t * t * t + 1.;
            },
            Self::InOutQuint => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return 0.5 * (t * t * t * t * t + 2.);
                }
            },
            Self::InSine => {
                return -(t * PI * 0.5).cos() + 1.;
            },
            Self::OutSine => {
                return (t * PI * 0.5).sin();
            },
            Self::InOutSine => {
                return -0.5 * ((t * PI).cos() - 1.);
            },
            Self::InExp => {
                if t < 0.001 {
                    return 0.;
                }
                else {
                    return 2.0f64.powf(10. * (t - 1.));
                }
            },
            Self::OutExp => {
                if t > 0.999 {
                    return 1.;
                }
                else {
                    return -(2.0f64.powf(-10. * t)) + 1.;
                }
            },
            Self::InOutExp => {
                if t<0.001 {
                    return 0.;
                }
                if t>0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * 2.0f64.powf(10. * (t - 1.));
                }
                else {
                    let t = t - 1.;
                    return 0.5 * (-(2.0f64.powf(-10. * t)) + 2.);
                }
            },
            Self::InCirc => {
                return -((1. - t * t).sqrt() - 1.);
            },
            Self::OutCirc => {
                let t = t - 1.;
                return (1. - t * t).sqrt();
            },
            Self::InOutCirc => {
                let t = t * 2.;
                if t < 1. {
                    return -0.5 * ((1. - t * t).sqrt() - 1.);
                }
                else {
                    let t = t - 2.;
                    return 0.5 * ((1. - t * t).sqrt() + 1.);
                }
            },
            Self::InElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t - 1.0;
                return -(2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
            },
            Self::OutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                                
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                return 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
            },
            Self::InOutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    let t = t - 1.0;
                    return -0.5 * (2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
                }
                else {
                    let t = t - 1.0;
                    return 0.5 * 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
                }
            },
            Self::InBack => {
                let s = 1.70158;
                return t * t * ((s + 1.) * t - s);
            },
            Self::OutBack => {
                let s = 1.70158;
                let t = t - 1.;
                return t * t * ((s + 1.) * t + s) + 1.;
            },
            Self::InOutBack => {
                let s = 1.70158;
                let t = t * 2.0;
                if t < 1. {
                    let s = s * 1.525;
                    return 0.5 * (t * t * ((s + 1.) * t - s));
                }
                else {
                    let t = t - 2.;
                    return 0.5 * (t * t * ((s + 1.) * t + s) + 2.);
                }
            },
            Self::InBounce => {
                return 1.0 - Self::OutBounce.map(1.0 - t);
            },
            Self::OutBounce => {
                if t < (1. / 2.75) {
                    return 7.5625 * t * t;
                }
                if t < (2. / 2.75) {
                    let t = t - (1.5 / 2.75);
                    return 7.5625 * t * t + 0.75;
                }
                if t < (2.5 / 2.75) {
                    let t = t - (2.25 / 2.75);
                    return 7.5625 * t * t + 0.9375;
                }
                let t = t - (2.625 / 2.75);
                return 7.5625 * t * t + 0.984375;
            },
            Self::InOutBounce => {
                if t <0.5 {
                    return Self::InBounce.map(t * 2.) * 0.5;
                }
                else {
                    return Self::OutBounce.map(t * 2. - 1.) * 0.5 + 0.5;
                }
            },
            Self::Bezier {cp0, cp1, cp2, cp3} => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                                
                if (cp0 - cp1).abs() < 0.001 && (cp2 - cp3).abs() < 0.001 {
                    return t;
                }
                                
                let epsilon = 1.0 / 200.0 * t;
                let cx = 3.0 * cp0;
                let bx = 3.0 * (cp2 - cp0) - cx;
                let ax = 1.0 - cx - bx;
                let cy = 3.0 * cp1;
                let by = 3.0 * (cp3 - cp1) - cy;
                let ay = 1.0 - cy - by;
                let mut u = t;
                                
                for _i in 0..6 {
                    let x = ((ax * u + bx) * u + cx) * u - t;
                    if x.abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }
                    let d = (3.0 * ax * u + 2.0 * bx) * u + cx;
                    if d.abs() < 1e-6 {
                        break;
                    }
                    u = u - x / d;
                };
                                
                if t > 1. {
                    return (ay + by) + cy;
                }
                if t < 0. {
                    return 0.0;
                }
                                
                let mut w = 0.0;
                let mut v = 1.0;
                u = t;
                for _i in 0..8 {
                    let x = ((ax * u + bx) * u + cx) * u;
                    if (x - t).abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }
                                        
                    if t > x {
                        w = u;
                    }
                    else {
                        v = u;
                    }
                    u = (v - w) * 0.5 + w;
                }
                                
                return ((ay * u + by) * u + cy) * u;
            }
        }
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
    pub fn as_ref(&self)->ToWasmMsgRef<'_>{self.msg.as_ref_at(self.offset)}
}
