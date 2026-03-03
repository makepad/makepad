use crate::cursor::MouseCursor;
use crate::keyboard::{KeyEvent, TextInputEvent};
use crate::mouse::KeyModifiers;
use crate::shared_framebuf::{PresentableDraw, SharedSwapchain};
use makepad_error_log::LogLevel;
use makepad_micro_serde::*;

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct EventSample {
    pub event_u32: u32,
    pub event_meta: u64,
    pub start: f64,
    pub end: f64,
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct GPUSample {
    pub start: f64,
    pub end: f64,
    pub draw_calls: u64,
    pub instances: u64,
    pub vertices: u64,
    pub instance_bytes: u64,
    pub uniform_bytes: u64,
    pub vertex_buffer_bytes: u64,
    pub texture_bytes: u64,
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct GCSample {
    pub start: f64,
    pub end: f64,
    pub heap_live: u64,
}

#[derive(Debug, Clone)]
pub enum LocalProfileSample {
    Event(EventSample),
    GPU(GPUSample),
    GC(GCSample),
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct StudioLogItem {
    pub file_name: String,
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,
    pub message: String,
    pub explanation: Option<String>,
    pub level: LogLevel,
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct JumpToFile {
    pub file_name: String,
    pub line: u32,
    pub column: u32,
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct PatchFile {
    pub file_name: String,
    pub line: u32,
    pub column_start: u32,
    pub column_end: u32,
    pub undo_group: u64,
    pub replace: String,
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct EditFile {
    pub file_name: String,
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,
    pub replace: String,
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct SelectInFile {
    pub file_name: String,
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct SwapSelection {
    pub s1_file_name: String,
    pub s1_line_start: u32,
    pub s1_line_end: u32,
    pub s1_column_start: u32,
    pub s1_column_end: u32,
    pub s2_file_name: String,
    pub s2_line_start: u32,
    pub s2_line_end: u32,
    pub s2_column_start: u32,
    pub s2_column_end: u32,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteKeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool,
}

impl RemoteKeyModifiers {
    pub fn into_key_modifiers(&self) -> KeyModifiers {
        KeyModifiers {
            shift: self.shift,
            control: self.control,
            alt: self.alt,
            logo: self.logo,
        }
    }

    pub fn from_key_modifiers(km: &KeyModifiers) -> Self {
        Self {
            shift: km.shift,
            control: km.control,
            alt: km.alt,
            logo: km.logo,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteMouseDown {
    pub button_raw_bits: u32,
    pub x: f64,
    pub y: f64,
    pub time: f64,
    pub modifiers: RemoteKeyModifiers,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteMouseMove {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub modifiers: RemoteKeyModifiers,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteTweakRay {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub modifiers: RemoteKeyModifiers,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteMouseUp {
    pub time: f64,
    pub button_raw_bits: u32,
    pub x: f64,
    pub y: f64,
    pub modifiers: RemoteKeyModifiers,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteTextInput {
    pub time: f64,
    pub window_id: usize,
    pub raw_button: usize,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteScroll {
    pub time: f64,
    pub sx: f64,
    pub sy: f64,
    pub x: f64,
    pub y: f64,
    pub is_mouse: bool,
    pub modifiers: RemoteKeyModifiers,
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub enum AppToStudio {
    LogItem(StudioLogItem),
    EventSample(EventSample),
    GPUSample(GPUSample),
    GCSample(GCSample),
    JumpToFile(JumpToFile),
    SelectInFile(SelectInFile),
    PatchFile(PatchFile),
    EditFile(EditFile),
    SwapSelection(SwapSelection),
    Screenshot(ScreenshotResponse),
    WidgetTreeDump(WidgetTreeDumpResponse),
    WidgetQuery(WidgetQueryResponse),
    TweakHits(TweakHitsResponse),
    BeforeStartup,
    CreateWindow { window_id: usize, kind_id: usize },
    AfterStartup,
    RequestAnimationFrame,
    SetCursor(MouseCursor),
    SetClipboard(String),
    // the client is done drawing, and the texture is completely updated
    DrawCompleteAndFlip(PresentableDraw),
    /// Application-defined response to a `StudioToApp::Custom` event.
    Custom(String),
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct ScreenshotResponse {
    pub request_ids: Vec<u64>,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct WidgetTreeDumpRequest {
    pub request_id: u64,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct WidgetTreeDumpResponse {
    pub request_id: u64,
    pub dump: String,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct WidgetQueryRequest {
    pub request_id: u64,
    pub query: String,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct WidgetQueryResponse {
    pub request_id: u64,
    pub query: String,
    pub rects: Vec<String>,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct TweakHitsResponse {
    pub window_id: usize,
    pub dpi_factor: f64,
    pub ray_x: f64,
    pub ray_y: f64,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub widget_uids: Vec<u64>,
}

#[derive(SerBin, DeBin, SerJson, DeJson)]
pub struct AppToStudioVec(pub Vec<AppToStudio>);

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct ScreenshotRequest {
    pub request_id: u64,
    pub kind_id: u32,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub enum StudioToApp {
    Screenshot(ScreenshotRequest),
    WidgetTreeDump(WidgetTreeDumpRequest),
    WidgetQuery(WidgetQueryRequest),
    KeepAlive,
    LiveChange {
        file_name: String,
        content: String,
    },
    Swapchain(SharedSwapchain),
    WindowGeomChange {
        dpi_factor: f64,
        window_id: usize,
        left: f64,
        top: f64,
        width: f64,
        height: f64,
    },
    Tick,
    MouseDown(RemoteMouseDown),
    MouseUp(RemoteMouseUp),
    MouseMove(RemoteMouseMove),
    TweakRay(RemoteTweakRay),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    TextCopy,
    TextCut,
    Scroll(RemoteScroll),
    /// Application-defined event. Delivered to the app as `Event::Custom`.
    Custom(String),
    #[default]
    None,
    Kill,
}

#[derive(SerBin, DeBin, SerJson, DeJson)]
pub struct StudioToAppVec(pub Vec<StudioToApp>);

impl AppToStudio {
    pub fn to_json(&self) -> String {
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl StudioToApp {
    pub fn to_json(&self) -> String {
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}
