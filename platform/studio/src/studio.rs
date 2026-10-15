use crate::cursor::MouseCursor;
use crate::hub_protocol::FrameCodec;
use crate::keyboard::{KeyEvent, TextInputEvent};
use crate::mouse::KeyModifiers;
use crate::relay::{ChildRelay, HostRelay};
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

/// Files dropped on the app's window by its host (the window manager): the
/// app sees `Event::Drop` of file-path items at `(x, y)`.
#[derive(Clone, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteDrop {
    pub x: f64,
    pub y: f64,
    pub paths: Vec<String>,
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

/// A gamepad's state, forwarded from Studio to the app it is hosting.
///
/// An app running under Studio is a child process with no window of its own,
/// so the OS delivers game-controller input to Studio and never to it — the
/// same reason mouse and key events are forwarded rather than read directly.
/// Sticks are flattened to scalars so the wire protocol stays independent of
/// the math crate, matching the other Remote* structs here.
#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteGamepad {
    pub a: f32,
    pub b: f32,
    pub x: f32,
    pub y: f32,
    pub left_shoulder: f32,
    pub right_shoulder: f32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub select: f32,
    pub start: f32,
    pub home: f32,
    pub left_thumb: f32,
    pub right_thumb: f32,
    pub dpad_up: f32,
    pub dpad_down: f32,
    pub dpad_left: f32,
    pub dpad_right: f32,
    pub left_stick_x: f32,
    pub left_stick_y: f32,
    pub right_stick_x: f32,
    pub right_stick_y: f32,
}

/// A racing wheel's state. Carried alongside gamepads so a wheel does not
/// silently vanish under Studio while a pad keeps working.
#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteWheel {
    pub steering: f32,
    pub throttle: f32,
    pub brake: f32,
    pub clutch: f32,
    pub steer_force: f32,
    pub buttons: u32,
}

/// A flight stick's state (see the platform's `JoystickState`).
#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemoteJoystick {
    pub x: f32,
    pub y: f32,
    pub twist: f32,
    pub throttle: f32,
    pub hat: u8,
    pub buttons: u32,
}

#[derive(Clone, Copy, Debug, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub enum RemoteGameInput {
    Gamepad(RemoteGamepad),
    Wheel(RemoteWheel),
    Joystick(RemoteJoystick),
}

impl Default for RemoteGameInput {
    fn default() -> Self {
        Self::Gamepad(RemoteGamepad::default())
    }
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
    /// The trackpad gesture phase (`None` for a wheel), so a hosted app's
    /// scrollers follow the host's momentum stream and rubber-band like local ones.
    pub phase: ScrollPhase,
}

/// The gesture phase of a scroll event, when the OS provides one (the platform's
/// `ScrollEvent`, shared here like `PinchPhase` so a remote scroll keeps it).
///
/// Trackpad scrolling on some platforms (macOS in particular) is a gesture: the OS reports
/// when fingers touch the pad (`Began`), each movement while they are down (`Changed`), when
/// they lift off (`Ended`), and then a stream of decaying momentum deltas
/// (`Momentum`/`MomentumEnded`).
///
/// The scrollable widgets apply the user-driven deltas directly and, on `Ended`, start a fling
/// that follows the OS momentum stream so the deceleration matches the OS (see
/// `widgets::scroll_motion`). Widgets that don't track phases can apply every delta directly,
/// which gives plain OS momentum scrolling.
///
/// The Linux direct backend synthesizes the same phases from the touchpad's multitouch
/// reports, momentum stream included (`linux_touchpad`), so widgets behave the same there.
///
/// On platforms and devices with no phase information (classic mouse wheels, X11, Windows),
/// this is `None`.
#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq, Eq)]
pub enum ScrollPhase {
    /// No phase information available (classic mouse wheel, or the platform doesn't report phases).
    #[default]
    None,
    /// Fingers touched the trackpad; a scroll gesture may begin (the delta is usually zero).
    Began,
    /// A raw finger contact on the trackpad, sent for every touch with a zero delta
    /// (macOS and the Linux direct backend). Unlike `Began`, this fires even for single-finger contacts that never
    /// become a scroll gesture, so widgets use it to instantly stop kinetic scrolling the
    /// way a touch natively catches a coast. It must not disturb anything else: any
    /// tap-to-click press or drag from the same contact arrives separately as mouse events.
    Touched,
    /// Fingers moved while on the trackpad: a user-driven scroll delta.
    Changed,
    /// Fingers lifted off the trackpad: the last event of the user-driven gesture (the delta
    /// may be zero). Widgets start their fling here.
    Ended,
    /// A momentum delta arriving from the OS (or the direct backend) after the fingers lifted. Widgets follow these to
    /// match the OS deceleration rate.
    Momentum,
    /// The OS momentum stream finished (the delta is zero).
    MomentumEnded,
}

/// The phase of a trackpad pinch (the platform's `PinchEvent`, shared here
/// like `KeyModifiers` so a remote pinch needs no conversion).
#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq, Eq)]
pub enum PinchPhase {
    /// Two fingers started pinching; the scale is 1.
    #[default]
    Begin,
    /// The fingers moved; the scale is the change since the previous event.
    Update,
    /// The fingers lifted, or the system cancelled the gesture; the scale is 1.
    End,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct RemotePinch {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    /// Multiplicative, relative to the previous event of the gesture.
    pub scale: f64,
    pub phase: PinchPhase,
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
    RunViewFrame(RunViewFrameData),
    RunViewKeyFocusRect(RunViewKeyFocusRect),
    WidgetTreeDump(WidgetTreeDumpResponse),
    WidgetQuery(WidgetQueryResponse),
    WidgetSnapshot(WidgetSnapshotResponse),
    TweakHits(TweakHitsResponse),
    BeforeStartup,
    CreateWindow {
        window_id: usize,
        kind_id: usize,
    },
    AfterStartup,
    RequestAnimationFrame,
    SetCursor(MouseCursor),
    SetClipboard(String),
    // the client is done drawing, and the texture is completely updated
    DrawCompleteAndFlip(PresentableDraw),
    /// Application-defined response to a `StudioToApp::Custom` event.
    Custom(String),
    /// The child consumed one `StudioToApp::Tick` (timers, draw, repaint).
    /// The host paces its next Tick on this, so a slow child never has
    /// more than one frame's worth of ticks and pointer moves queued.
    TickDone,
    /// A system service the child cannot reach itself (no OS window, no
    /// JVM): the host performs it and answers with `StudioToApp::Relay`.
    /// APPENDED LAST (ordinal tags, see `StudioToApp::MouseCancel`); only
    /// children that know their host serves relays send it.
    Relay(ChildRelay),
    /// The child is idle and needs no Tick for `secs` seconds (its earliest
    /// timer). APPENDED LAST (ordinal tags).
    WakeIn(f64),
}

#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub struct ScreenshotResponse {
    pub request_ids: Vec<u64>,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct RunViewFrameRequest {
    pub window_id: usize,
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub dpi_factor: f64,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct RunViewFrameData {
    pub window_id: usize,
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub codec: Option<FrameCodec>,
    pub data: Vec<u8>,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct RunViewKeyFocusRect {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
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

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone, PartialEq, Eq)]
pub struct WidgetSnapshot {
    pub id: String,
    pub widget_type: String,
    pub window_id: String,
    pub window_index: usize,
    pub visible: bool,
    pub enabled: bool,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub text: Option<String>,
    pub value: Option<String>,
    pub checked: Option<bool>,
    pub selected: Option<String>,
    /// The dotted id path `/snap` reports as `p` (`main.new_note`): the names
    /// of the widget and its named ancestors below the window. None for an
    /// unnamed widget.
    pub path: Option<String>,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct WidgetSnapshotRequest {
    pub request_id: u64,
}

#[derive(Debug, Default, SerBin, DeBin, SerJson, DeJson, Clone)]
pub struct WidgetSnapshotResponse {
    pub request_id: u64,
    pub widgets: Vec<WidgetSnapshot>,
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
    RunViewFrameRequest(RunViewFrameRequest),
    WidgetTreeDump(WidgetTreeDumpRequest),
    WidgetQuery(WidgetQueryRequest),
    WidgetSnapshot(WidgetSnapshotRequest),
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
    Pinch(RemotePinch),
    /// The full set of game controllers Studio can see, resent whenever it
    /// changes. Level state rather than edges, because that is what the OS
    /// APIs report and what `Cx::game_input_states` hands back.
    GameInput(Vec<RemoteGameInput>),
    /// Application-defined event. Delivered to the app as `Event::Custom`.
    Custom(String),
    #[default]
    None,
    Kill,
    /// The host took the press away (its own gesture claimed the finger, or
    /// it rotated/closed the view): the app ends the press as a
    /// cancellation — no click, no fling — and releases the button.
    ///
    /// APPENDED LAST on purpose: the binary encoding tags variants by
    /// ordinal, so every existing variant keeps its tag. A child built
    /// before this variant cannot decode it and skips it with an error
    /// (`StudioToAppVec::decode_bin`); the rest of its batch still arrives.
    MouseCancel(RemoteMouseUp),
    /// The host's answer to an `AppToStudio::Relay` (an HTTP response, a
    /// permission result, a picked file). APPENDED LAST.
    Relay(HostRelay),
    /// Files dropped on the window by the host (APPENDED LAST): delivered as `Event::Drag`
    /// (so a widget can light its drop target) then `Event::Drop`, `Event::DragEnd`.
    DropFiles(RemoteDrop),
}

/// A batch of host messages. On the wire every message is framed with its
/// byte length, so one message an app cannot decode (a host built from a
/// different tree: a new variant, a changed struct) costs that message only,
/// with a clear error, never the rest of the batch. A message whose frame is
/// longer than what this app reads (fields the host appended last) decodes,
/// and the extra bytes are skipped.
#[derive(SerJson, DeJson)]
pub struct StudioToAppVec(pub Vec<StudioToApp>);

/// Marks a framed batch, so another payload on the same socket (a hub hello)
/// is never mistaken for one.
const STUDIO_BATCH_MAGIC: [u8; 4] = *b"MPB1";

impl SerBin for StudioToAppVec {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        s.extend_from_slice(&STUDIO_BATCH_MAGIC);
        (self.0.len() as u64).ser_bin(s);
        for msg in &self.0 {
            let at = s.len();
            0u64.ser_bin(s);
            msg.ser_bin(s);
            let len = (s.len() - at - 8) as u64;
            s[at..at + 8].copy_from_slice(&len.to_le_bytes());
        }
    }
}

impl StudioToAppVec {
    /// Decodes a batch: the messages this app understands, and one error line
    /// per message it could not decode (the others still arrive). `Err` only
    /// when the payload is not a framed batch at all.
    pub fn decode_bin(d: &[u8]) -> Result<(Self, Vec<String>), DeBinErr> {
        let fail = |msg: &str, o: usize| DeBinErr { msg: msg.to_string(), o, l: 0, s: d.len() };
        if d.len() < 12 || d[..4] != STUDIO_BATCH_MAGIC {
            return Err(fail("StudioToAppVec: not a framed batch", 0));
        }
        let mut o = 4;
        let count = u64::de_bin(&mut o, d)? as usize;
        let mut msgs = Vec::with_capacity(count.min(4096));
        let mut errors = Vec::new();
        for index in 0..count {
            let len = u64::de_bin(&mut o, d)? as usize;
            let Some(frame) = d.get(o..o.saturating_add(len)) else {
                return Err(fail("StudioToAppVec: a frame runs past the payload", o));
            };
            match StudioToApp::de_bin(&mut 0, frame) {
                Ok(msg) => msgs.push(msg),
                Err(error) => errors.push(format!(
                    "host message {} of {} ({} bytes) could not be decoded and was skipped: {:?} -- \
                     the host and this app were built from different trees; rebuild the app",
                    index + 1, count, len, error
                )),
            }
            o += len;
        }
        if o != d.len() {
            return Err(fail("StudioToAppVec: bytes after the last frame", o));
        }
        Ok((Self(msgs), errors))
    }
}

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

/// Visibility, host to app. A host that shows none of this window (minimized,
/// on another space, fully covered by other windows) sends `visible: false`;
/// `true` says some of it shows again. A hidden child delivers no
/// next-frame callbacks and draws nothing -- the screen cannot show it --
/// and asks for no frames; on `true` it redraws once and resumes. It
/// travels as `StudioToApp::Custom`, read by `HostedVisible::parse(json)` in
/// the child's platform (a native app's Cx, the WM-hosted HAL shell of
/// `makepad run`), and never reaches the app.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct HostedVisible { pub visible: bool }
#[derive(SerJson, DeJson)]
struct VisibleEnvelope { makepad_visible: HostedVisible }
impl HostedVisible {
    pub fn to_json(&self) -> String {
        VisibleEnvelope { makepad_visible: self.clone() }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_visible\"") { return None; }
        VisibleEnvelope::deserialize_json(json).ok().map(|e| e.makepad_visible)
    }
}

/// Appearance, host to app: whether the window system is dark (the WM's color scheme, the one
/// its portal gives browsers). Sent on connect and on every change, as `StudioToApp::Custom`.
/// Read by what draws without a stylesheet: `makepad run`'s load screen through the WM-hosted
/// HAL shell (`makepad_hal::host_dark`); a widgets app follows the `makepad_style` sheet.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedAppearance { pub dark: bool }
#[derive(SerJson, DeJson)]
struct AppearanceEnvelope { makepad_appearance: HostedAppearance }
impl HostedAppearance {
    pub fn to_json(&self) -> String { AppearanceEnvelope { makepad_appearance: self.clone() }.serialize_json() }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_appearance\"") { return None; }
        AppearanceEnvelope::deserialize_json(json).ok().map(|e| e.makepad_appearance)
    }
}

// Own chrome (the contract is described in the platform's `ime.rs`): the WM asks a makepad
// window's drag query on a press and the app answers; the app's window buttons ask the WM for
// the window op. Both travel as `Custom` JSON. They live here, not in the platform, because the
// WM-hosted HAL shell of `makepad run` relays them too (`platform/hal/src/backend/linux/wm.rs`).

/// `HostedDragAnswer::response` values (a `WindowDragQueryResponse`).
pub mod drag_answer {
    pub const NO_ANSWER: u8 = 0;
    pub const CLIENT: u8 = 1;
    pub const CAPTION: u8 = 2;
    pub const SYS_MENU: u8 = 3;
}

/// Host to app: where did a press land (the window's own points)?
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedDragQuery { pub id: u32, pub x: f64, pub y: f64 }
#[derive(SerJson, DeJson)]
struct DragQueryEnvelope { makepad_drag_query: HostedDragQuery }
impl HostedDragQuery {
    pub fn to_json(&self) -> String { DragQueryEnvelope { makepad_drag_query: self.clone() }.serialize_json() }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_drag_query\"") { return None; }
        DragQueryEnvelope::deserialize_json(json).ok().map(|e| e.makepad_drag_query)
    }
}

/// `HostedDragAnswer::double_click` values: what a double click on the caption does
/// (the window's `CxWindow::double_click_fullscreen`).
pub mod drag_double {
    /// Maximise to the screen's work area (bars and dock stay), again restores.
    pub const MAXIMISE: u8 = 0;
    /// Fullscreen on its screen (over bars and dock, no caption), again restores.
    pub const FULLSCREEN: u8 = 1;
}

/// App to host: the answer to a [`HostedDragQuery`].
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedDragAnswer { pub id: u32, pub response: u8, pub double_click: u8 }
#[derive(SerJson, DeJson)]
struct DragAnswerEnvelope { makepad_drag_answer: HostedDragAnswer }
impl HostedDragAnswer {
    pub fn to_json(&self) -> String { DragAnswerEnvelope { makepad_drag_answer: self.clone() }.serialize_json() }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_drag_answer\"") { return None; }
        DragAnswerEnvelope::deserialize_json(json).ok().map(|e| e.makepad_drag_answer)
    }
}

/// `HostedWindowOp::op` values: what a window of the app asked for.
pub mod window_op {
    pub const CLOSE: u8 = 0;
    pub const MINIMIZE: u8 = 1;
    /// The maximise button. A hosted app cannot see whether the host has the
    /// window maximised, so the host decides: it maximises, or restores a
    /// window that already is.
    pub const MAXIMIZE: u8 = 2;
    /// Leave the maximised state; nothing when not maximised.
    pub const RESTORE: u8 = 3;
}

/// App to host: the app's window was asked to close, minimise or
/// maximise/restore (`Window::close` and the like). A hosted child has no
/// platform window to do it to, so the host, which owns the window, does it
/// with its own paths. Sent by every hosted backend, so any app with its own
/// caption buttons gets them working.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedWindowOp { pub op: u8 }
#[derive(SerJson, DeJson)]
struct WindowOpEnvelope { makepad_window_op: HostedWindowOp }
impl HostedWindowOp {
    pub fn to_json(&self) -> String { WindowOpEnvelope { makepad_window_op: self.clone() }.serialize_json() }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_window_op\"") { return None; }
        WindowOpEnvelope::deserialize_json(json).ok().map(|e| e.makepad_window_op)
    }
}

#[cfg(test)]
mod studio_batch_tests {
    use super::*;

    fn moves() -> Vec<StudioToApp> {
        (0..3).map(|i| StudioToApp::MouseMove(RemoteMouseMove { x: i as f64, ..Default::default() })).collect()
    }

    fn xs(batch: &StudioToAppVec) -> Vec<f64> {
        batch.0.iter().map(|m| match m { StudioToApp::MouseMove(e) => e.x, _ => -1.0 }).collect()
    }

    #[test]
    fn a_batch_round_trips() {
        let bytes = StudioToAppVec(moves()).serialize_bin();
        let (batch, errors) = StudioToAppVec::decode_bin(&bytes).unwrap();
        assert_eq!(xs(&batch), [0.0, 1.0, 2.0]);
        assert!(errors.is_empty());
    }

    /// Frame offsets of a serialized batch: (start of the message, its length).
    fn frames(bytes: &[u8]) -> Vec<(usize, usize)> {
        let count = u64::from_le_bytes(bytes[4..12].try_into().unwrap()) as usize;
        let mut o = 12;
        (0..count).map(|_| {
            let len = u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap()) as usize;
            let frame = (o + 8, len);
            o += 8 + len;
            frame
        }).collect()
    }

    #[test]
    fn an_unknown_message_costs_only_itself_with_a_clear_error() {
        let mut bytes = StudioToAppVec(moves()).serialize_bin();
        // The middle message gets a variant tag this app does not know (a newer host).
        let (at, _) = frames(&bytes)[1];
        bytes[at..at + 2].copy_from_slice(&0xfff0u16.to_le_bytes());
        let (batch, errors) = StudioToAppVec::decode_bin(&bytes).unwrap();
        assert_eq!(xs(&batch), [0.0, 2.0]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("host message 2 of 3"), "{}", errors[0]);
    }

    #[test]
    fn fields_a_newer_host_appended_are_skipped() {
        // A host whose message carries one more trailing field than this app reads.
        let mut msg = StudioToApp::MouseMove(RemoteMouseMove { x: 7.0, ..Default::default() }).serialize_bin();
        msg.extend_from_slice(&3u32.to_le_bytes());
        let mut bytes = STUDIO_BATCH_MAGIC.to_vec();
        2u64.ser_bin(&mut bytes);
        for _ in 0..2 {
            (msg.len() as u64).ser_bin(&mut bytes);
            bytes.extend_from_slice(&msg);
        }
        let (batch, errors) = StudioToAppVec::decode_bin(&bytes).unwrap();
        assert_eq!(xs(&batch), [7.0, 7.0]);
        assert!(errors.is_empty());
    }

    #[test]
    fn other_payloads_are_not_batches() {
        assert!(StudioToAppVec::decode_bin(&StudioToApp::Tick.serialize_bin()).is_err());
        let mut bytes = StudioToAppVec(moves()).serialize_bin();
        bytes.pop();
        assert!(StudioToAppVec::decode_bin(&bytes).is_err(), "a truncated batch");
    }
}
