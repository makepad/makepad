#![allow(dead_code)]
use crate::{
    makepad_live_id::*,
    makepad_wasm_bridge::*,
    makepad_math::{Vec2, Vec3, Quat, Transform},
    midi::{MidiInputInfo, Midi1Data, Midi1InputData},
    cx::{PlatformType},
    window::CxWindowPool,
    area::Area,
    event::{
        DigitId,
        XRButton,
        XRInput,
        XRUpdateEvent,
        KeyCode,
        FingerDownEvent,
        KeyModifiers,
        FingerInputType,
        FingerUpEvent,
        FingerMoveEvent,
        FingerHoverEvent,
        FingerScrollEvent,
        KeyEvent,
        TextInputEvent,
        MidiInputListEvent,
    },
    window::WindowGeom
};



#[derive(ToWasm)]
pub struct WVec2 {
    pub x: f32,
    pub y: f32,
}

impl Into<Vec2> for WVec2 {
    fn into(self) -> Vec2 {
        Vec2 {x: self.x, y: self.y}
    }
}


#[derive(ToWasm)]
pub struct WGpuInfo {
    pub min_uniform_vectors: u32,
    pub vendor: String,
    pub renderer: String
}

#[derive(ToWasm)]
pub struct WBrowserInfo {
    pub protocol: String,
    pub hostname: String,
    pub host: String,
    pub pathname: String,
    pub search: String,
    pub hash: String,
    pub has_thread_support: bool,
}

impl Into<PlatformType> for WBrowserInfo {
    fn into(self) -> PlatformType {
        PlatformType::WebBrowser {
            protocol: self.protocol,
            hostname: self.hostname,
            host:self.host,
            pathname: self.pathname,
            search: self.search,
            hash: self.hash,
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmGetDeps {
    pub gpu_info: WGpuInfo,
    pub browser_info: WBrowserInfo,
}

#[derive(ToWasm)]
pub struct WDepLoaded {
    pub path: String,
    pub data: WasmDataU8
}

#[derive(ToWasm)]
pub struct WindowInfo {
    pub is_fullscreen: bool,
    pub can_fullscreen: bool,
    pub xr_is_presenting: bool,
    pub xr_can_present: bool,
    pub dpi_factor: f32,
    pub inner_width: f32,
    pub inner_height: f32
}

impl Into<WindowGeom> for WindowInfo {
    fn into(self) -> WindowGeom {
        WindowGeom {
            is_fullscreen: self.is_fullscreen,
            is_topmost: false,
            inner_size: Vec2 {x: self.inner_width, y: self.inner_height},
            dpi_factor: self.dpi_factor,
            outer_size: Vec2 {x: 0., y: 0.},
            position: Vec2 {x: 0., y: 0.},
            xr_is_presenting: self.xr_is_presenting,
            xr_can_present: self.xr_can_present,
            can_fullscreen: self.can_fullscreen
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmInit {
    pub deps: Vec<WDepLoaded>,
    pub window_info: WindowInfo
}

#[derive(ToWasm)]
pub struct ToWasmResizeWindow {
    pub window_info: WindowInfo
}

#[derive(ToWasm)]
pub struct ToWasmAnimationFrame {
    pub time: f64
}



// Finger API



#[derive(ToWasm)]
pub struct WTouch {
    pub x: f32,
    pub y: f32,
    pub uid: u32,
    pub modifiers: u32,
    pub time: f64,
}

#[derive(ToWasm)]
pub struct ToWasmTouchStart {
    pub touch: WTouch,
}

fn unpack_key_modifier(modifiers: u32) -> KeyModifiers {
    KeyModifiers {
        shift: (modifiers & 1) != 0,
        control: (modifiers & 2) != 0,
        alt: (modifiers & 4) != 0,
        logo: (modifiers & 8) != 0
    }
}

impl ToWasmTouchStart {
    pub fn into_finger_down_event(self, digit_id:DigitId, tap_count: u32) -> FingerDownEvent {
        FingerDownEvent {
            window_id: CxWindowPool::id_zero(),
            abs: Vec2 {x: self.touch.x, y: self.touch.y},
            handled: false,
            digit_id,
            input_type: FingerInputType::Touch(self.touch.uid as usize),
            modifiers: KeyModifiers::default(),
            time: self.touch.time,
            tap_count: tap_count
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmTouchMove {
    pub touch: WTouch,
}

impl ToWasmTouchMove {
    pub fn into_finger_move_event(self, digit_id:DigitId, captured:Area ) -> FingerMoveEvent {
        FingerMoveEvent {
            window_id: CxWindowPool::id_zero(),
            abs: Vec2 {x: self.touch.x, y: self.touch.y},
            digit_id,
            captured,
            input_type: FingerInputType::Touch(self.touch.uid as usize),
            modifiers: KeyModifiers::default(),
            time: self.touch.time,
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmTouchEnd {
    pub touch: WTouch,
}

impl ToWasmTouchEnd {
    pub fn into_finger_up_event(self, digit_id:DigitId, captured:Area ) -> FingerUpEvent {
        FingerUpEvent {
            window_id: CxWindowPool::id_zero(),
            abs: Vec2 {x: self.touch.x, y: self.touch.y},
            digit_id,
            captured,
            input_type: FingerInputType::Touch(self.touch.uid as usize),
            modifiers: KeyModifiers::default(),
            time: self.touch.time,
        }
    }
}


// Mouse API


#[derive(ToWasm)]
pub struct WMouse {
    pub x: f32,
    pub y: f32,
    pub modifiers: u32,
    pub button: u32,
    pub time: f64,
}

#[derive(ToWasm)]
pub struct ToWasmMouseDown {
    pub mouse: WMouse,
}
impl ToWasmMouseDown {
    pub fn into_finger_down_event(self, digit_id:DigitId, tap_count: u32) -> FingerDownEvent {
        FingerDownEvent {
            window_id: CxWindowPool::id_zero(),
            abs: Vec2 {x: self.mouse.x, y: self.mouse.y},
            handled: false,
            digit_id,
            input_type: FingerInputType::Mouse(self.mouse.button as usize),
            modifiers: unpack_key_modifier(self.mouse.modifiers),
            time: self.mouse.time,
            tap_count: tap_count
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmMouseMove {
    pub was_out: bool,
    pub mouse: WMouse,
}

impl ToWasmMouseMove {
    pub fn into_finger_move_event(self, digit_id:DigitId, captured:Area) -> FingerMoveEvent {
        FingerMoveEvent {
            window_id: CxWindowPool::id_zero(),
            abs: Vec2 {x: self.mouse.x, y: self.mouse.y},
            digit_id,
            captured,
            input_type: FingerInputType::Mouse(self.mouse.button as usize),
            modifiers: unpack_key_modifier(self.mouse.modifiers),
            time: self.mouse.time,
        }
    }
}

impl ToWasmMouseMove {
    pub fn into_finger_hover_event(self, digit_id:DigitId, hover_last:Area) -> FingerHoverEvent {
        FingerHoverEvent {
            window_id: CxWindowPool::id_zero(),
            abs: Vec2 {x: self.mouse.x, y: self.mouse.y},
            handled: false,
            hover_last,
            digit_id,
            input_type: FingerInputType::Mouse(self.mouse.button as usize),
            modifiers: unpack_key_modifier(self.mouse.modifiers),
            time: self.mouse.time,
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmMouseUp {
    pub mouse: WMouse,
}

impl ToWasmMouseUp {
    pub fn into_finger_up_event(self, digit_id:DigitId, captured:Area) -> FingerUpEvent {
        FingerUpEvent {
            window_id: CxWindowPool::id_zero(),
            abs: Vec2 {x: self.mouse.x, y: self.mouse.y},
            captured,
            digit_id,
            input_type: FingerInputType::Mouse(self.mouse.button as usize),
            modifiers: unpack_key_modifier(self.mouse.modifiers),
            time: self.mouse.time,
        }
    }
}

// scroll

#[derive(ToWasm)]
pub struct ToWasmScroll {
    pub x: f32,
    pub y: f32,
    pub modifiers: u32,
    pub is_touch: bool,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub time: f64
}

impl ToWasmScroll {
    pub fn into_finger_scroll_event(self, digit_id:DigitId) -> FingerScrollEvent {
        FingerScrollEvent{
            window_id: CxWindowPool::id_zero(),
            digit_id,
            abs: Vec2 {x: self.x, y: self.y},
            scroll: Vec2 {x: self.scroll_x, y: self.scroll_y},
            input_type: if self.is_touch {FingerInputType::Touch(0)} else {FingerInputType::Mouse(0)},
            handled_x: false,
            handled_y: false,
            modifiers: unpack_key_modifier(self.modifiers),
            time: self.time,
        }
    }
}

/*
#[derive(ToWasm)]
pub struct ToWasmMouseOut {
    pub mouse: WMouse,
}

impl Into<FingerHoverEvent> for ToWasmMouseOut {
    fn into(self) -> FingerHoverEvent {
        FingerHoverEvent {
            window_id: CxWindowPool::id_zero(),
            abs: Vec2 {x: self.mouse.x, y: self.mouse.y},
            handled: false,
            digit: 0,
            input_type: FingerInputType::Mouse(self.mouse.button),
            modifiers: unpack_key_modifier(self.mouse.modifiers),
            time: self.mouse.time,
        }
    }
}*/



fn web_to_key_code(key_code: u32) -> KeyCode {
    match key_code {
        27 => KeyCode::Escape,
        192 => KeyCode::Backtick,
        48 => KeyCode::Key0,
        49 => KeyCode::Key1,
        50 => KeyCode::Key2,
        51 => KeyCode::Key3,
        52 => KeyCode::Key4,
        53 => KeyCode::Key5,
        54 => KeyCode::Key6,
        55 => KeyCode::Key7,
        56 => KeyCode::Key8,
        57 => KeyCode::Key9,
        173 => KeyCode::Minus,
        189 => KeyCode::Minus,
        61 => KeyCode::Equals,
        187 => KeyCode::Equals,
        
        8 => KeyCode::Backspace,
        9 => KeyCode::Tab,
        
        81 => KeyCode::KeyQ,
        87 => KeyCode::KeyW,
        69 => KeyCode::KeyE,
        82 => KeyCode::KeyR,
        84 => KeyCode::KeyT,
        89 => KeyCode::KeyY,
        85 => KeyCode::KeyU,
        73 => KeyCode::KeyI,
        79 => KeyCode::KeyO,
        80 => KeyCode::KeyP,
        219 => KeyCode::LBracket,
        221 => KeyCode::RBracket,
        13 => KeyCode::ReturnKey,
        
        65 => KeyCode::KeyA,
        83 => KeyCode::KeyS,
        68 => KeyCode::KeyD,
        70 => KeyCode::KeyF,
        71 => KeyCode::KeyG,
        72 => KeyCode::KeyH,
        74 => KeyCode::KeyJ,
        75 => KeyCode::KeyK,
        76 => KeyCode::KeyL,
        
        59 => KeyCode::Semicolon,
        186 => KeyCode::Semicolon,
        222 => KeyCode::Quote,
        220 => KeyCode::Backslash,
        
        90 => KeyCode::KeyZ,
        88 => KeyCode::KeyX,
        67 => KeyCode::KeyC,
        86 => KeyCode::KeyV,
        66 => KeyCode::KeyB,
        78 => KeyCode::KeyN,
        77 => KeyCode::KeyM,
        188 => KeyCode::Comma,
        190 => KeyCode::Period,
        191 => KeyCode::Slash,
        
        17 => KeyCode::Control,
        18 => KeyCode::Alt,
        16 => KeyCode::Shift,
        224 => KeyCode::Logo,
        91 => KeyCode::Logo,
        
        //RightControl,
        //RightShift,
        //RightAlt,
        93 => KeyCode::Logo,
        
        32 => KeyCode::Space,
        20 => KeyCode::Capslock,
        112 => KeyCode::F1,
        113 => KeyCode::F2,
        114 => KeyCode::F3,
        115 => KeyCode::F4,
        116 => KeyCode::F5,
        117 => KeyCode::F6,
        118 => KeyCode::F7,
        119 => KeyCode::F8,
        120 => KeyCode::F9,
        121 => KeyCode::F10,
        122 => KeyCode::F11,
        123 => KeyCode::F12,
        
        44 => KeyCode::PrintScreen,
        124 => KeyCode::PrintScreen,
        //Scrolllock,
        //Pause,
        
        45 => KeyCode::Insert,
        46 => KeyCode::Delete,
        36 => KeyCode::Home,
        35 => KeyCode::End,
        33 => KeyCode::PageUp,
        34 => KeyCode::PageDown,
        
        96 => KeyCode::Numpad0,
        97 => KeyCode::Numpad1,
        98 => KeyCode::Numpad2,
        99 => KeyCode::Numpad3,
        100 => KeyCode::Numpad4,
        101 => KeyCode::Numpad5,
        102 => KeyCode::Numpad6,
        103 => KeyCode::Numpad7,
        104 => KeyCode::Numpad8,
        105 => KeyCode::Numpad9,
        
        //NumpadEquals,
        109 => KeyCode::NumpadSubtract,
        107 => KeyCode::NumpadAdd,
        110 => KeyCode::NumpadDecimal,
        106 => KeyCode::NumpadMultiply,
        111 => KeyCode::NumpadDivide,
        12 => KeyCode::Numlock,
        //NumpadEnter,
        
        38 => KeyCode::ArrowUp,
        40 => KeyCode::ArrowDown,
        37 => KeyCode::ArrowLeft,
        39 => KeyCode::ArrowRight,
        _ => KeyCode::Unknown
    }
}

#[derive(ToWasm, Clone)]
pub struct WKey {
    pub char_code: u32,
    pub key_code: u32,
    pub modifiers: u32,
    pub time: f64,
    pub is_repeat: bool
}

impl Into<KeyEvent> for WKey {
    fn into(self) -> KeyEvent {
        KeyEvent {
            key_code: web_to_key_code(self.key_code),
            is_repeat: self.is_repeat,
            modifiers: unpack_key_modifier(self.modifiers),
            time: self.time,
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmKeyDown {
    pub key: WKey
}

#[derive(ToWasm)]
pub struct ToWasmKeyUp {
    pub key: WKey
}


#[derive(ToWasm)]
pub struct ToWasmTextInput {
    pub was_paste: bool,
    pub replace_last: bool,
    pub input: String,
}

impl Into<TextInputEvent> for ToWasmTextInput {
    fn into(self) -> TextInputEvent {
        TextInputEvent {
            was_paste: self.was_paste,
            replace_last: self.replace_last,
            input: self.input
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmTextCopy {
}

#[derive(ToWasm)]
pub struct ToWasmTimerFired {
    pub timer_id: usize
}

#[derive(ToWasm)]
pub struct ToWasmPaintDirty {
}

#[derive(ToWasm)]
pub struct ToWasmRedrawAll {}

#[derive(ToWasm, Clone)]
pub struct WVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Into<Vec3> for WVec3 {
    fn into(self) -> Vec3 {
        Vec3 {x: self.x, y: self.y, z: self.z}
    }
}

#[derive(ToWasm, Clone)]
pub struct WQuat {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

impl Into<Quat> for WQuat {
    fn into(self) -> Quat {
        Quat {a: self.a, b: self.b, c: self.c, d: self.d}
    }
}

#[derive(ToWasm, Clone)]
pub struct WXRButton {
    pub pressed: bool,
    pub value: f32
}

#[derive(ToWasm, Clone)]
pub struct WXRTransform {
    pub orientation: WQuat,
    pub position: WVec3,
}

impl Into<Transform> for WXRTransform {
    fn into(self) -> Transform {
        Transform {
            orientation: self.orientation.into(),
            position: self.position.into()
        }
    }
}

impl Into<XRButton> for WXRButton {
    fn into(self) -> XRButton {
        XRButton {
            value: self.value,
            pressed: self.pressed
        }
    }
}

#[derive(ToWasm)]
pub struct WXRInput {
    pub active: bool,
    pub hand: u32,
    pub grip: WXRTransform,
    pub ray: WXRTransform,
    pub buttons: Vec<WXRButton>,
    pub axes: Vec<f32>
}

impl Into<XRInput> for WXRInput {
    fn into(self) -> XRInput {
        XRInput {
            active: self.active,
            hand: self.hand,
            grip: self.grip.into(),
            ray: self.ray.into(),
            axes: self.axes,
            buttons: self.buttons.into_iter().map( | v | v.into()).collect(),
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmXRUpdate {
    pub time: f64,
    pub head_transform: WXRTransform,
    pub inputs: Vec<WXRInput>,
}

impl ToWasmXRUpdate {
    pub fn into_xrupdate_event(self, last_inputs: Option<Vec<XRInput >>) -> XRUpdateEvent {
        XRUpdateEvent {
            time: self.time,
            head_transform: self.head_transform.into(),
            inputs: self.inputs.into_iter().map( | v | v.into()).collect(),
            last_inputs
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmAppGotFocus {}

#[derive(ToWasm)]
pub struct ToWasmAppLostFocus {}

#[derive(ToWasm)]
pub struct WSignal {
    pub signal_hi: u32,
    pub signal_lo: u32,
}

#[derive(ToWasm)]
pub struct ToWasmSignal {
    pub signals: Vec<WSignal>
}

#[derive(ToWasm)]
pub struct ToWasmWebSocketClose {
    pub web_socket_id: usize
}

#[derive(ToWasm)]
pub struct ToWasmWebSocketOpen {
    pub web_socket_id: usize
}

#[derive(ToWasm)]
pub struct ToWasmWebSocketError {
    pub web_socket_id: usize,
    pub error: String
}

#[derive(ToWasm)]
pub struct ToWasmWebSocketMessage {
    pub web_socket_id: usize,
    pub data: WasmDataU8
}

#[derive(ToWasm)]
pub struct ToWasmMidiInputData {
    pub input_id: u32,
    pub data: u32,
}

impl Into<Midi1InputData> for ToWasmMidiInputData {
    fn into(self) -> Midi1InputData {
        Midi1InputData {
            input_id: self.input_id as usize,
            data: Midi1Data{
                data0: ((self.data>>16)&0xff) as u8,
                data1: ((self.data>>8)&0xff) as u8 ,
                data2: ((self.data>>0)&0xff) as u8,
            }
        }
    }
}

#[derive(ToWasm)]
pub struct WMidiInputInfo {
    pub manufacturer: String,
    pub name: String,
    pub uid: String,
}



#[derive(ToWasm)]
pub struct ToWasmMidiInputList {
    pub inputs:Vec<WMidiInputInfo>
}

impl Into<MidiInputListEvent> for ToWasmMidiInputList {
    fn into(self) -> MidiInputListEvent {
        let mut out = Vec::new();
        for input in self.inputs{
            out.push(MidiInputInfo {
                manufacturer: input.manufacturer,
                name: input.name,
                uid: input.uid
            });
        }
        MidiInputListEvent{inputs:out}
    }
}
