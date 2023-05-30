use {
    std::cell::Cell,
    crate::{
        makepad_live_id::*,
        makepad_wasm_bridge::*,
        makepad_math::{dvec2, DVec2, Vec3, Quat, Transform},
        cx::{OsType, XrCapabilities, WebParams},
        window::CxWindowPool,
        area::Area,
        //midi::{MidiData},
        event::{
            XRButton,
            XRInput,
            XRUpdateEvent,
            KeyCode,
            KeyModifiers,
            MouseDownEvent,
            MouseMoveEvent,
            MouseUpEvent,
            TouchPoint,
            TouchState,
            TouchUpdateEvent,
            ScrollEvent,
            KeyEvent,
            TextInputEvent,
            WindowGeom
        },
    }
};

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

impl Into<OsType> for WBrowserInfo {
    fn into(self) -> OsType {
        OsType::Web(WebParams{
            protocol: self.protocol,
            hostname: self.hostname,
            host: self.host,
            pathname: self.pathname,
            search: self.search,
            hash: self.hash,
        })
    }
}

#[derive(ToWasm)]
pub struct ToWasmGetDeps {
    pub gpu_info: WGpuInfo,
    pub cpu_cores: u32,
    pub xr_capabilities: WXrCapabilities,
    pub browser_info: WBrowserInfo,
}

#[derive(ToWasm)]
pub struct WDepLoaded {
    pub path: String,
    pub data: WasmDataU8
}

#[derive(ToWasm)]
pub struct WWindowInfo {
    pub is_fullscreen: bool,
    pub can_fullscreen: bool,
    pub xr_is_presenting: bool,
    pub vr_supported: bool,
    pub ar_supported: bool,
    pub dpi_factor: f64,
    pub inner_width: f64,
    pub inner_height: f64
}

impl Into<WindowGeom> for WWindowInfo {
    fn into(self) -> WindowGeom {
        WindowGeom {
            is_fullscreen: self.is_fullscreen,
            is_topmost: false,
            inner_size: DVec2 {x: self.inner_width, y: self.inner_height},
            dpi_factor: self.dpi_factor,
            outer_size: DVec2 {x: 0., y: 0.},
            position: DVec2 {x: 0., y: 0.},
            xr_is_presenting: self.xr_is_presenting,
            can_fullscreen: self.can_fullscreen
        }
    }
}

#[derive(ToWasm)]
pub struct WXrCapabilities {
    pub vr_supported: bool,
    pub ar_supported: bool,
}

impl Into<XrCapabilities> for WXrCapabilities {
    fn into(self) -> XrCapabilities {
        XrCapabilities {
            vr_supported: self.vr_supported,
            ar_supported: self.ar_supported,
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmInit {
    pub deps: Vec<WDepLoaded>,
    pub window_info: WWindowInfo
}

#[derive(ToWasm)]
pub struct ToWasmResizeWindow {
    pub window_info: WWindowInfo
}

#[derive(ToWasm)]
pub struct ToWasmAnimationFrame {
    pub time: f64
}

#[derive(ToWasm)]
pub struct ToWasmTimerFired {
    pub timer_id: usize
}

#[derive(ToWasm)]
pub struct ToWasmSignal {
}

#[derive(ToWasm)]
pub struct ToWasmPaintDirty {
}

#[derive(ToWasm)]
pub struct ToWasmRedrawAll {}





// Touch API



#[derive(ToWasm, Clone, Debug)]
pub struct WTouchPoint {
    pub state: u32, // 0 stable, 1 start, 2 end, 3 move
    pub x: f64,
    pub y: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    pub rotation_angle: f64,
    pub force: f64,
    pub uid: u32,
}

impl From<WTouchPoint> for TouchPoint {
    fn from(v:WTouchPoint) -> Self {
        Self {
            abs: dvec2(v.x, v.y),
            state: match v.state{
                0=>TouchState::Stable,
                1=>TouchState::Start,
                2=>TouchState::Move,
                _=>TouchState::Stop,
            },
            uid: v.uid as u64,
            force: v.force,
            radius: dvec2(v.radius_x, v.radius_y),
            rotation_angle: v.rotation_angle,
            sweep_lock: Cell::new(Area::Empty),
            handled: Cell::new(Area::Empty),
        }
    }
}

#[derive(ToWasm, Debug)]
pub struct ToWasmTouchUpdate {
    pub modifiers: u32,
    pub time: f64,
    pub touches: Vec<WTouchPoint>,
}

impl From<ToWasmTouchUpdate> for TouchUpdateEvent {
    fn from(v:ToWasmTouchUpdate) -> Self {
        Self {
            time: v.time,
            window_id: CxWindowPool::id_zero(),
            modifiers: unpack_key_modifier(v.modifiers),
            touches: v.touches.iter().map(|v| v.clone().into()).collect()
        }
    }
}

fn unpack_key_modifier(modifiers: u32) -> KeyModifiers {
    KeyModifiers {
        shift: (modifiers & 1) != 0,
        control: (modifiers & 2) != 0,
        alt: (modifiers & 4) != 0,
        logo: (modifiers & 8) != 0
    }
}


// Mouse API


#[derive(ToWasm)]
pub struct WMouse {
    pub x: f64,
    pub y: f64,
    pub modifiers: u32,
    pub button: u32,
    pub time: f64,
}

#[derive(ToWasm)]
pub struct ToWasmMouseDown {
    pub mouse: WMouse,
}

impl From<ToWasmMouseDown> for MouseDownEvent {
    fn from(v:ToWasmMouseDown) -> Self {
        Self {
            abs: dvec2(v.mouse.x, v.mouse.y),
            button: v.mouse.button as usize,
            window_id: CxWindowPool::id_zero(),
            modifiers: unpack_key_modifier(v.mouse.modifiers),
            handled: Cell::new(Area::Empty),
            time: v.mouse.time
        }
    }
}

#[derive(ToWasm)]
pub struct ToWasmMouseMove {
    pub was_out: bool,
    pub mouse: WMouse,
}

impl From<ToWasmMouseMove> for MouseMoveEvent {
    fn from(v:ToWasmMouseMove) -> Self {
        Self {
            abs: dvec2(v.mouse.x, v.mouse.y),
            window_id: CxWindowPool::id_zero(),
            modifiers: unpack_key_modifier(v.mouse.modifiers),
            handled: Cell::new(Area::Empty),
            time: v.mouse.time
        }
    }
}


#[derive(ToWasm)]
pub struct ToWasmMouseUp {
    pub mouse: WMouse,
}

impl From<ToWasmMouseUp> for MouseUpEvent {
    fn from(v:ToWasmMouseUp) -> Self {
        Self {
            abs: dvec2(v.mouse.x, v.mouse.y),
            button: v.mouse.button as usize,
            window_id: CxWindowPool::id_zero(),
            modifiers: unpack_key_modifier(v.mouse.modifiers),
            time: v.mouse.time
        }
    }
}

// scroll

#[derive(ToWasm)]
pub struct ToWasmScroll {
    pub x: f64,
    pub y: f64,
    pub modifiers: u32,
    pub is_touch: bool,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub time: f64
}

impl From<ToWasmScroll> for ScrollEvent {
    fn from(v:ToWasmScroll) -> Self {
        Self {
            window_id: CxWindowPool::id_zero(),
            scroll: dvec2(v.scroll_x, v.scroll_y),
            abs: dvec2(v.x, v.y),
            modifiers: unpack_key_modifier(v.modifiers),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: true,
            time: v.time,
        }
    }
}

// Keyboard API


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

// Keyboard API



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
    pub hand: u32,
    pub active: bool,
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
pub struct ToWasmHTTPResponse {
    pub id: String,
    pub status: u32,
    pub headers: String,
    pub body: WasmDataU8
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
    pub uid: String,
    pub data: u32,
}

#[derive(ToWasm)]
pub struct WMidiPortInfo {
    pub name: String,
    pub uid: String,
    pub is_output: bool
}

#[derive(ToWasm)]
pub struct ToWasmMidiPortList {
    pub ports: Vec<WMidiPortInfo>,
}

#[derive(ToWasm)]
pub struct WAudioDevice {
    pub web_device_id: String,
    pub label: String,
    pub is_output: bool,
}

#[derive(ToWasm)]
pub struct ToWasmAudioDeviceList {
    pub devices: Vec<WAudioDevice>
}
