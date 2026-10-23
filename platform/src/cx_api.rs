use crate::file_dialogs::{FileDialog, VirtualFileLimits};

use {
    crate::{
        area::Area,
        cursor::MouseCursor,
        cx::{Cx, CxRef, GpuBackend, OsType, XrCapabilities},
        display_context::{ScreenOrientation, SystemBarAppearance},
        draw_list::DrawListId,
        draw_pass::{CxDrawPassParent, CxDrawPassRect, DrawPassId},
        dvec2,
        event::keyboard::CharOffset,
        event::xr::XrAnchor,
        event::{
            video_playback::CameraPreviewMode, DigitId, DragItem, Event, NextFrame, QuitReason,
            QuitRequestedEvent, Timer, Trigger, VideoSource,
        },
        gpu_info::GpuInfo,
        ime::TextInputConfig,
        macos_menu::MacosMenu,
        makepad_futures::executor::Spawner,
        makepad_live_id::*,
        makepad_math::{Rect, Vec2d},
        makepad_network::HttpRequest,
        makepad_script::value::ScriptHandle,
        shared_bytes::SharedBytes,
        texture::{Texture, TextureId},
        window::WindowVisuals,
        window::{CxWindow, WindowId},
    },
    std::{
        any::{Any, TypeId},
        ops::Range,
        rc::Rc,
    },
};
pub enum OpenUrlInPlace {
    Yes,
    No,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CxThreadPriority {
    #[default]
    Normal,
    UserInteractive,
    UserInitiated,
    Utility,
    Background,
    Idle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrFrameCpuBreakdown {
    pub total_ms: f64,
    pub wait_frame_ms: f64,
    pub begin_frame_ms: f64,
    pub locate_space_ms: f64,
    pub locate_views_ms: f64,
    pub acquire_swapchain_ms: f64,
    pub wait_swapchain_ms: f64,
    pub acquire_depth_ms: f64,
    pub update_prepare_ms: f64,
    pub update_dispatch_ms: f64,
    pub next_frame_ms: f64,
    pub draw_event_ms: f64,
    pub compile_shaders_ms: f64,
    pub repaint_ms: f64,
    pub repaint_wait_inflight_ms: f64,
    pub repaint_prepare_textures_ms: f64,
    pub repaint_record_draw_ms: f64,
    pub repaint_submit_ms: f64,
    pub repaint_texture_upload_count: u32,
    pub repaint_texture_upload_bytes: u64,
    pub repaint_packet_buffer_count: u32,
    pub repaint_packet_buffer_bytes: u64,
    pub repaint_geometry_upload_bytes: u64,
    pub repaint_descriptor_set_count: u32,
    pub repaint_draw_items: u64,
    pub repaint_draw_calls: u64,
    pub repaint_packets: u64,
    pub repaint_instances: u64,
    pub repaint_indices: u64,
    pub depth_readback_ms: f64,
    pub end_frame_ms: f64,
    pub resize_projection_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SystemBrowserId(pub LiveId);

impl From<LiveId> for SystemBrowserId {
    fn from(value: LiveId) -> Self {
        Self(value)
    }
}

pub struct CxSystemBrowser<'a> {
    cx: &'a mut Cx,
    id: SystemBrowserId,
}

impl<'a> CxSystemBrowser<'a> {
    pub fn id(&self) -> SystemBrowserId {
        self.id
    }

    pub fn spawn(&mut self, url: &str) {
        if self.cx.script_data.std.host_io_only() { return; }
        self.cx.platform_ops.push_back(CxOsOp::SpawnSystemBrowser {
            browser_id: self.id.0,
            url: url.to_string(),
        });
    }

    pub fn update(&mut self, area: Area, visible: bool) {
        self.cx.platform_ops.push_back(CxOsOp::UpdateSystemBrowser {
            browser_id: self.id.0,
            area,
            visible,
        });
    }

    pub fn detach(&mut self) {
        self.cx.platform_ops.push_back(CxOsOp::DetachSystemBrowser {
            browser_id: self.id.0,
        });
    }

    pub fn set_url(&mut self, url: &str, replace: bool) {
        if self.cx.script_data.std.host_io_only() { return; }
        self.cx.platform_ops.push_back(CxOsOp::SetSystemBrowserUrl {
            browser_id: self.id.0,
            url: url.to_string(),
            replace,
        });
    }

    pub fn history_go(&mut self, delta: i32) {
        if self.cx.script_data.std.host_io_only() { return; }
        self.cx
            .platform_ops
            .push_back(CxOsOp::SystemBrowserHistoryGo {
                browser_id: self.id.0,
                delta,
            });
    }

    pub fn close(&mut self) {
        self.cx.platform_ops.push_back(CxOsOp::CloseSystemBrowser {
            browser_id: self.id.0,
        });
    }
}

/// A system haptic (`Cx::haptic_feedback`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapticFeedback { Click, VirtualKey, Tick }

pub trait CxOsApi {
    fn init_cx_os(&mut self);

    fn start_stdin_service(&mut self) {}
    fn pre_start() -> bool {
        false
    }

    fn open_url(&mut self, url: &str, in_place: OpenUrlInPlace);

    fn browser_update_url(&mut self, _url: &str, _replace: bool) {}

    fn browser_history_go(&mut self, _delta: i32) {}

    /// Platform hook shared by native and synthetic pointer injection. Cocoa
    /// uses it to activate the clicked window before the down is dispatched.
    fn activate_window_on_pointer_down(&mut self, _window_id: WindowId) {}

    /// Re-evaluate any platform pacing override after widget capture state
    /// may have changed in a pointer callback.
    fn update_pointer_capture_pacing(&mut self) {}

    fn seconds_since_app_start(&self) -> f64;

    fn default_window_size(&self) -> Vec2d {
        dvec2(800., 600.)
    }

    fn max_texture_width() -> usize {
        4096
    }

    fn in_xr_mode(&self) -> bool {
        false
    }

    fn micro_zbias_step(&self) -> f32 {
        0.00001
    }

    fn xr_render_scale(&self) -> Option<f64> {
        None
    }

    fn xr_gpu_frame_time_ms(&self) -> Option<f64> {
        None
    }

    fn xr_frame_cpu_time_ms(&self) -> Option<f64> {
        None
    }

    fn xr_render_cpu_time_ms(&self) -> Option<f64> {
        None
    }

    fn xr_depth_readback_cpu_time_ms(&self) -> Option<f64> {
        None
    }

    fn xr_frame_cpu_breakdown(&self) -> Option<XrFrameCpuBreakdown> {
        None
    }

    fn xr_display_refresh_rate_hz(&self) -> Option<f64> {
        None
    }

    fn xr_effective_frame_rate_hz(&self) -> Option<f64> {
        None
    }

    /*
    fn web_socket_open(&mut self, url: String, rec: WebSocketAutoReconnect) -> WebSocket;
    fn web_socket_send(&mut self, socket: WebSocket, data: Vec<u8>);*/
}

/// Type-erased accessibility tree update payload. PartialEq always returns
/// false — accessibility updates are never deduplicated.
pub struct AccessibilityUpdatePayload(pub Box<dyn std::any::Any + Send>);

impl PartialEq for AccessibilityUpdatePayload {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for AccessibilityUpdatePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AccessibilityUpdatePayload(..)")
    }
}

/// A set of screen edges, for [`Cx::defer_system_gestures`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenEdges(u8);

impl ScreenEdges {
    pub const NONE: ScreenEdges = ScreenEdges(0);
    pub const TOP: ScreenEdges = ScreenEdges(1);
    pub const LEFT: ScreenEdges = ScreenEdges(2);
    pub const BOTTOM: ScreenEdges = ScreenEdges(4);
    pub const RIGHT: ScreenEdges = ScreenEdges(8);
    pub const ALL: ScreenEdges = ScreenEdges(15);

    pub const fn with(self, other: ScreenEdges) -> ScreenEdges {
        ScreenEdges(self.0 | other.0)
    }

    pub const fn contains(self, other: ScreenEdges) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[derive(PartialEq)]
pub enum CxOsOp {
    // ---- child windows and domains of the HAL client (HAL.md 5c / 5d)
    /// Resizes a child's embedded window (points) and sets its dpi.
    #[cfg(not(target_arch = "wasm32"))]
    ChildGeom { child: u32, window: crate::event::LentId, size: Vec2d, dpi: f64 },
    /// An event for a child, in the child window's coordinates: `makepad_hal::buffers::pack_forward`.
    #[cfg(not(target_arch = "wasm32"))]
    ForwardEvent { child: u32, window: crate::event::LentId, packed: Vec<u8> },
    /// The child holds the keyboard, IME, cursor and clipboard; its siblings lose them.
    #[cfg(not(target_arch = "wasm32"))]
    FocusChild(u32),
    /// Asks the child to close its window.
    #[cfg(not(target_arch = "wasm32"))]
    CloseChildWindow { child: u32, window: crate::event::LentId },
    #[cfg(not(target_arch = "wasm32"))]
    StartDomain { request: u32, app: String, args: String },
    #[cfg(not(target_arch = "wasm32"))]
    StopDomain { domain: u32, kill: bool },
    #[cfg(not(target_arch = "wasm32"))]
    RelaunchDomain(u32),
    #[cfg(not(target_arch = "wasm32"))]
    ListDomains(u32),
    #[cfg(not(target_arch = "wasm32"))]
    SetOwnChrome(bool),

    CreateWindow(WindowId),
    CreatePopupWindow {
        window_id: WindowId,
        parent_window_id: WindowId,
        position: Vec2d,
        size: Vec2d,
        grab_keyboard: bool,
    },
    ResizeWindow(WindowId, Vec2d),
    RepositionWindow(WindowId, Vec2d),
    CloseWindow(WindowId),
    MinimizeWindow(WindowId),
    Deminiaturize(WindowId),
    MaximizeWindow(WindowId),
    FullscreenWindow(WindowId),
    NormalizeWindow(WindowId),
    RestoreWindow(WindowId),
    HideWindow(WindowId),
    HideWindowButtons(WindowId),
    ShowWindowButtons(WindowId),
    /// Centre the native window buttons (macOS traffic lights) on this
    /// distance below the window's top edge, in points: a caption row of
    /// another height than the system's keeps them in line. No-op elsewhere.
    CenterWindowButtons(WindowId, f64),
    SetTopmost(WindowId, bool),
    /// `true`: drop the native maximized-state border/titlebar strip a
    /// backend would otherwise keep (Windows only; other backends ignore
    /// it) so a maximized window reads as a clean fullscreen picture
    /// rather than a decorated window pinned to the work area.
    SetChromelessWhenMaximized(WindowId, bool),
    SetWindowTitle(WindowId, String),
    SetWindowVisuals(WindowId, WindowVisuals),
    /// macOS only: the dpi a window lays its UI out at instead of the native one,
    /// which keeps the traffic lights centered in a caption bar that zooms with the UI.
    SetWindowDpiOverride(WindowId, Option<f64>),
    ShowInDock(bool),
    /// Bring the app and its windows to the front, restoring a minimized
    /// one where the backend can (a relaunch of a single-instance app).
    /// Honored on macOS, Windows and X11; a `--remote` or hidden instance
    /// never takes focus.
    ActivateApp,
    /// FPS-style pointer lock: `true` hides the cursor and freezes it in
    /// place while mouse deltas keep arriving (as synthesized absolute
    /// positions, so existing MouseMove consumers work unchanged); `false`
    /// releases. Backends without support ignore it.
    LockMousePointer(bool),
    /// Widget-scoped pointer pin for value scrubbing: cursor stays at its
    /// press point (hidden) while deltas keep flowing; restored in place
    /// on release. Engage at the drag threshold, never on the press.
    PinMousePointer(bool),
    /// Per-frame lock maintenance, pushed by the captured app every frame:
    /// re-pins the hardware cursor. Exists because OS-level disassociation
    /// proves unreliable on some systems (it silently drops on app
    /// deactivation and other events) — SDL's fallback is the same
    /// warp-every-frame. No-op when unlocked or unsupported.
    RepinMousePointer,
    /// Tints the system bar (status/navigation bar) icons: `true` requests
    /// dark icons, `false` requests light icons. Honored on Android and iOS
    /// (iOS only has a status bar).
    SetSystemBarDarkIcons(bool),
    /// Locks the screen orientation, or releases the lock. Honored on
    /// Android and iOS.
    SetScreenOrientation(ScreenOrientation),
    /// Keeps the display from dimming and sleeping while `true`. Honored on
    /// Android and iOS.
    SetKeepScreenOn(bool),
    /// Asks the OS to defer its own edge gestures (the home-indicator swipe,
    /// the status-bar and control-centre pulls) on these screen edges, so
    /// the first swipe from such an edge reaches the app. Honored on iOS.
    DeferSystemGestures(ScreenEdges),

    // The `Rect` is the caret/composition line's bounding box (top-left + size,
    // line height included), so each backend can ask the OS to place the IME
    // candidate window directly above/below the line instead of over it.
    ShowTextIME(Area, Rect, TextInputConfig),
    HideTextIME,
    SyncImeState {
        text: String,
        selection: Range<CharOffset>,
        composition: Option<Range<CharOffset>>,
    },
    SetCursor(MouseCursor),
    StartTimer {
        timer_id: u64,
        interval: f64,
        repeats: bool,
    },
    StopTimer(u64),
    Quit,

    StartDragging(Vec<DragItem>),
    /// Begin an operating-system drag whose destination may be another
    /// application. This is deliberately separate from `StartDragging`:
    /// several Makepad widgets use that operation for the framework's
    /// immediate, synthetic in-window drag protocol.
    StartExternalDragging {
        window_id: WindowId,
        items: Vec<DragItem>,
    },
    UpdateMacosMenu(MacosMenu),
    ShowClipboardActions {
        has_selection: bool,
        rect: Rect,
        keyboard_shift: f64,
    },
    HideClipboardActions,
    CopyToClipboard(String),
    ShareText(String),
    SetPrimarySelection(String),
    ShowSelectionHandles {
        start: Vec2d,
        end: Vec2d,
    },
    UpdateSelectionHandles {
        start: Vec2d,
        end: Vec2d,
    },
    HideSelectionHandles,
    AccessibilityUpdate(AccessibilityUpdatePayload),

    CheckPermission {
        permission: crate::permission::Permission,
        request_id: i32,
    },
    RequestPermission {
        permission: crate::permission::Permission,
        request_id: i32,
    },

    StartLocationUpdates,
    StopLocationUpdates,

    /// A system haptic (Android under the HAL).
    #[cfg(target_os = "android")]
    HapticFeedback(HapticFeedback),

    HttpRequest {
        request_id: LiveId,
        request: HttpRequest,
    },
    CancelHttpRequest {
        request_id: LiveId,
    },
    #[cfg(target_arch = "wasm32")]
    #[allow(private_interfaces)]
    StorageRequest(crate::storage::StorageRequest),
    #[cfg(target_arch = "wasm32")]
    StorageRequestError {
        request_id: crate::storage::StorageRequestId,
        op: crate::storage::StorageOp,
        error: crate::storage::StorageError,
    },

    PrepareVideoPlayback(
        LiveId,
        VideoSource,
        CameraPreviewMode,
        u32,
        TextureId,
        bool,
        bool,
    ),
    AttachCameraNativePreview {
        video_id: LiveId,
        area: Area,
    },
    UpdateCameraNativePreview {
        video_id: LiveId,
        area: Area,
        visible: bool,
    },
    DetachCameraNativePreview {
        video_id: LiveId,
    },
    SpawnSystemBrowser {
        browser_id: LiveId,
        url: String,
    },
    UpdateSystemBrowser {
        browser_id: LiveId,
        area: Area,
        visible: bool,
    },
    DetachSystemBrowser {
        browser_id: LiveId,
    },
    SetSystemBrowserUrl {
        browser_id: LiveId,
        url: String,
        replace: bool,
    },
    SystemBrowserHistoryGo {
        browser_id: LiveId,
        delta: i32,
    },
    CloseSystemBrowser {
        browser_id: LiveId,
    },
    PrepareAudioPlayback(LiveId, VideoSource, bool, bool),
    BeginVideoPlayback(LiveId),
    PauseVideoPlayback(LiveId),
    ResumeVideoPlayback(LiveId),
    MuteVideoPlayback(LiveId),
    UnmuteVideoPlayback(LiveId),
    CleanupVideoPlaybackResources(LiveId),
    SeekVideoPlayback(LiveId, u64),
    SetVideoVolume(LiveId, f64),
    SetVideoPlaybackRate(LiveId, f64),
    SelectVideoTrack(LiveId, usize),
    SelectAudioTrack(LiveId, usize),

    CreateWebView {
        id: LiveId,
        area: Area,
        texture: Texture,
        url: String,
    },
    UpdateWebView {
        id: LiveId,
        area: Area,
    },
    CloseWebView {
        id: LiveId,
    },
    SaveFileDialog(FileDialog),
    SelectFileDialog(FileDialog),
    SaveFolderDialog(FileDialog),
    SelectFolderDialog(FileDialog),

    XrStartPresenting,
    XrSetRenderScale(f32),
    XrSetLocalAnchor(XrAnchor),
    XrSetLocalFloor(f32),
    XrAdvertiseAnchor(XrAnchor),
    XrDiscoverAnchor(u8),
    XrStopPresenting,
}

impl CxOsOp {
    /// The variant's name (what `Debug` prints; the payload is not shown).
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::ChildGeom { .. } => "ChildGeom",
            #[cfg(not(target_arch = "wasm32"))]
            Self::ForwardEvent { .. } => "ForwardEvent",
            #[cfg(not(target_arch = "wasm32"))]
            Self::FocusChild(..) => "FocusChild",
            #[cfg(not(target_arch = "wasm32"))]
            Self::CloseChildWindow { .. } => "CloseChildWindow",
            #[cfg(not(target_arch = "wasm32"))]
            Self::StartDomain { .. } => "StartDomain",
            #[cfg(not(target_arch = "wasm32"))]
            Self::StopDomain { .. } => "StopDomain",
            #[cfg(not(target_arch = "wasm32"))]
            Self::RelaunchDomain(..) => "RelaunchDomain",
            #[cfg(not(target_arch = "wasm32"))]
            Self::ListDomains(..) => "ListDomains",
            #[cfg(not(target_arch = "wasm32"))]
            Self::SetOwnChrome(..) => "SetOwnChrome",
            Self::CreateWindow(..) => "CreateWindow",
            Self::CreatePopupWindow { .. } => "CreatePopupWindow",
            Self::CloseWindow(..) => "CloseWindow",
            Self::MinimizeWindow(..) => "MinimizeWindow",
            Self::Deminiaturize(..) => "Deminiaturize",
            Self::MaximizeWindow(..) => "MaximizeWindow",
            Self::FullscreenWindow(..) => "FullscreenWindow",
            Self::NormalizeWindow(..) => "NormalizeWindow",
            Self::RestoreWindow(..) => "RestoreWindow",
            Self::HideWindow(..) => "HideWindow",
            Self::HideWindowButtons(..) => "HideWindowButtons",
            Self::ShowWindowButtons(..) => "ShowWindowButtons",
            Self::CenterWindowButtons(..) => "CenterWindowButtons",
            Self::SetTopmost(..) => "SetTopmost",
            Self::SetChromelessWhenMaximized(..) => "SetChromelessWhenMaximized",
            Self::SetWindowTitle(..) => "SetWindowTitle",
            Self::SetWindowVisuals(..) => "SetWindowVisuals",
            Self::SetWindowDpiOverride(..) => "SetWindowDpiOverride",
            Self::ShowInDock(..) => "ShowInDock",
            Self::ActivateApp => "ActivateApp",
            Self::LockMousePointer(..) => "LockMousePointer",
            Self::PinMousePointer(..) => "PinMousePointer",
            Self::RepinMousePointer => "RepinMousePointer",
            Self::SetSystemBarDarkIcons(..) => "SetSystemBarDarkIcons",
            Self::DeferSystemGestures(..) => "DeferSystemGestures",
            Self::SetScreenOrientation(..) => "SetScreenOrientation",
            Self::SetKeepScreenOn(..) => "SetKeepScreenOn",

            Self::ShowTextIME(..) => "ShowTextIME",
            Self::HideTextIME => "HideTextIME",
            Self::SyncImeState { .. } => "SyncImeState",
            Self::SetCursor(..) => "SetCursor",
            Self::StartTimer { .. } => "StartTimer",
            Self::StopTimer(..) => "StopTimer",
            Self::Quit => "Quit",

            Self::StartDragging(..) => "StartDragging",
            Self::StartExternalDragging { .. } => "StartExternalDragging",
            Self::UpdateMacosMenu(..) => "UpdateMacosMenu",
            Self::ShowClipboardActions { .. } => "ShowClipboardActions",
            Self::HideClipboardActions => "HideClipboardActions",
            Self::CopyToClipboard(..) => "CopyToClipboard",
            Self::ShareText(..) => "ShareText",
            Self::SetPrimarySelection(..) => "SetPrimarySelection",
            Self::ShowSelectionHandles { .. } => "ShowSelectionHandles",
            Self::UpdateSelectionHandles { .. } => "UpdateSelectionHandles",
            Self::HideSelectionHandles => "HideSelectionHandles",
            Self::AccessibilityUpdate(..) => "AccessibilityUpdate",

            Self::CheckPermission { .. } => "CheckPermission",
            Self::RequestPermission { .. } => "RequestPermission",
            Self::StartLocationUpdates => "StartLocationUpdates",
            Self::StopLocationUpdates => "StopLocationUpdates",
            #[cfg(target_os = "android")]
            Self::HapticFeedback(..) => "HapticFeedback",

            Self::HttpRequest { .. } => "HttpRequest",
            Self::CancelHttpRequest { .. } => "CancelHttpRequest",
            #[cfg(target_arch = "wasm32")]
            Self::StorageRequest(..) => "StorageRequest",
            #[cfg(target_arch = "wasm32")]
            Self::StorageRequestError { .. } => "StorageRequestError",

            Self::PrepareVideoPlayback(..) => "PrepareVideoPlayback",
            Self::AttachCameraNativePreview { .. } => "AttachCameraNativePreview",
            Self::UpdateCameraNativePreview { .. } => "UpdateCameraNativePreview",
            Self::DetachCameraNativePreview { .. } => "DetachCameraNativePreview",
            Self::SpawnSystemBrowser { .. } => "SpawnSystemBrowser",
            Self::UpdateSystemBrowser { .. } => "UpdateSystemBrowser",
            Self::DetachSystemBrowser { .. } => "DetachSystemBrowser",
            Self::SetSystemBrowserUrl { .. } => "SetSystemBrowserUrl",
            Self::SystemBrowserHistoryGo { .. } => "SystemBrowserHistoryGo",
            Self::CloseSystemBrowser { .. } => "CloseSystemBrowser",
            Self::PrepareAudioPlayback(..) => "PrepareAudioPlayback",
            Self::BeginVideoPlayback(..) => "BeginVideoPlayback",
            Self::PauseVideoPlayback(..) => "PauseVideoPlayback",
            Self::ResumeVideoPlayback(..) => "ResumeVideoPlayback",
            Self::MuteVideoPlayback(..) => "MuteVideoPlayback",
            Self::UnmuteVideoPlayback(..) => "UnmuteVideoPlayback",
            Self::CleanupVideoPlaybackResources(..) => "CleanupVideoPlaybackResources",
            Self::SeekVideoPlayback(..) => "SeekVideoPlayback",
            Self::SetVideoVolume(..) => "SetVideoVolume",
            Self::SetVideoPlaybackRate(..) => "SetVideoPlaybackRate",
            Self::SelectVideoTrack(..) => "SelectVideoTrack",
            Self::SelectAudioTrack(..) => "SelectAudioTrack",
            Self::CreateWebView { .. } => "CreateWebView",
            Self::UpdateWebView { .. } => "UpdateWebView",
            Self::CloseWebView { .. } => "CloseWebView",
            Self::SaveFileDialog(..) => "SaveFileDialog",
            Self::SelectFileDialog(..) => "SelectFileDialog",
            Self::SaveFolderDialog(..) => "SaveFolderDialog",
            Self::SelectFolderDialog(..) => "SelectFolderDialog",
            Self::ResizeWindow(..) => "ResizeWindow",
            Self::RepositionWindow(..) => "RepositionWindow",

            Self::XrStartPresenting => "XrStartPresenting",
            Self::XrSetRenderScale(_) => "XrSetRenderScale",
            Self::XrStopPresenting => "XrStopPresenting",
            Self::XrAdvertiseAnchor(_) => "XrAdvertiseAnchor",
            Self::XrSetLocalAnchor(_) => "XrSetLocalAnchor",
            Self::XrSetLocalFloor(_) => "XrSetLocalFloor",
            Self::XrDiscoverAnchor(_) => "XrDiscoverAnchor",
        }
    }
}

impl std::fmt::Debug for CxOsOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Cx {
    /// Update a named dynamic uniform on one retained draw item without
    /// invalidating its immutable instance publication.
    pub fn set_draw_item_uniform(
        &mut self,
        list: DrawListId,
        item: usize,
        name: LiveId,
        value: &[f32],
    ) -> bool {
        // A stale (list, item) after eviction or re-recording is a no-op,
        // never a UI-thread panic.
        let draw_items = &self.draw_lists[list].draw_items;
        let Some(shader) = (item < draw_items.len())
            .then(|| draw_items[item].draw_call())
            .flatten()
            .map(|call| call.draw_shader_id)
        else {
            return false;
        };
        let Some(input) = self.draw_shaders[shader.index]
            .mapping
            .dyn_uniforms
            .inputs
            .iter()
            .find(|input| input.id == name)
        else {
            return false;
        };
        let offset = input.offset;
        let len = input.slots.min(value.len());
        let draw_list = &mut self.draw_lists[list];
        let changed = draw_list.draw_items.set_dyn_uniform(
            item,
            shader,
            offset,
            &value[..len],
            &mut self.uniform_gen,
        );
        if changed {
            if let Some(pass) = draw_list.draw_pass_id {
                self.passes[pass].paint_dirty = true;
            }
        }
        changed
    }
}

impl Cx {
    pub fn in_draw_event(&self) -> bool {
        self.in_draw_event
    }

    /// Requests a deferred `Event::ScriptReapply` on the next event-loop
    /// iteration. The captured app value is re-applied with
    /// `Apply::ScriptReapply` — no `script_mod` re-run, so runtime
    /// `script_eval!` overrides on the heap are preserved. Widgets that
    /// reference shared heap objects (e.g. `mod.widgets.IMG_MSG_FIT`) pick
    /// up in-place mutations on this re-apply walk.
    ///
    /// Use this when the change you made is a runtime mutation of a shared
    /// heap object — typically a `script_eval!` override.
    pub fn request_script_reapply(&mut self) {
        self.pending_script_reapply = true;
    }

    /// Re-evaluate application Splash with a new stylesheet, preserving text and
    /// other imperative state through `Apply::ScriptReapply`.
    pub fn request_style_reload(&mut self) {
        self.pending_style_reload = true;
        self.pending_live_edit_request = true;
    }

    /// Requests a deferred `Event::LiveEdit` on the next event-loop iteration.
    /// The handler re-runs `script_mod` (re-evaluating any expressions that
    /// reference primitive heap values like `mod.widgets.SAFE_INSET_PAD_TOP`)
    /// and then re-applies the widget tree with `Apply::Rebake`, which
    /// preserves imperative runtime state since the DSL itself is unchanged.
    ///
    /// Use this only when a primitive heap value has changed and that value
    /// is consumed by `script_mod!` block expressions — those expressions are
    /// not re-evaluated by `Apply::ScriptReapply`. The re-run is still a full
    /// tree walk, so prefer `request_script_reapply` when the change can be
    /// modeled as a shared-heap-object mutation instead.
    pub fn request_live_edit(&mut self) {
        self.pending_live_edit_request = true;
    }

    /// Arm the VM's captured-error sink for the live-edit re-run that
    /// follows, so a hot-reloaded block that fails to run is reported to
    /// the caller as well as the log. Called by the `app_main!` expansion,
    /// which is why it is public.
    pub fn live_edit_capture_begin(&mut self) {
        if self.script_vm.is_none() {
            return;
        }
        self.with_vm(|vm| vm.capture_errors());
    }

    /// Take the errors the re-run raised: log them, and keep them for
    /// `take_live_edit_errors`. Public for the `app_main!` expansion.
    pub fn live_edit_capture_end(&mut self) {
        if self.script_vm.is_none() {
            return;
        }
        let errors = self.with_vm(|vm| vm.take_errors());
        for error in &errors {
            crate::error!("live edit: {}", error);
        }
        self.live_edit_errors = errors;
    }

    /// The script errors raised by the most recent live-edit re-run, if any.
    /// Taking them clears the record.
    pub fn take_live_edit_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.live_edit_errors)
    }

    /// The `Apply` variant the currently dispatching `Event::LiveEdit` should
    /// be re-applied with — `Reload` for a file-change hot reload, `Rebake`
    /// for a `request_live_edit()` re-bake. `app_main!` reads this; app code
    /// has no reason to.
    pub fn live_edit_apply(&self) -> crate::makepad_script::Apply {
        self.live_edit_apply.clone()
    }

    /// Remap an absolute coordinate from the OS-reported logical-point space
    /// into the layout's logical-point space when a `dpi_override` is active
    /// on the given window. No-op if no override is set or `os_dpi_factor`
    /// hasn't been recorded yet.
    ///
    /// Platform-specific event handlers call this on every `abs` field of
    /// pointer/touch/scroll events so input still hits the right widget when
    /// zoom is active. Android doesn't need it because it divides
    /// raw pixels by the override-aware `dpi_factor` at the source — so this
    /// helper is dead code on those builds, hence the `allow(dead_code)`.
    #[allow(dead_code)]
    pub(crate) fn dpi_override_scale(&self, pos: &mut Vec2d, window_id: WindowId) {
        // Native window callbacks can be delivered after a window has been
        // destroyed.  In that case their Cocoa back-pointer no longer names a
        // live Cx window.  Pointer events are best-effort, so never let a stale
        // callback index the generational pool (and abort the whole app).
        if !self.windows.is_valid(window_id) || !self.windows[window_id].is_created {
            return;
        }
        *pos = self.windows[window_id].remap_dpi_override(*pos);
    }

    pub fn update_safe_inset_script_values(&mut self, insets: crate::event::SafeAreaInsets) {
        use makepad_script::trap::NoTrap;
        let Some(vm) = self.script_vm.as_mut() else {
            return;
        };
        let widgets = vm.heap_mut().module(id!(widgets));
        vm.heap_mut().set_value(
            widgets,
            id!(SAFE_INSET_PAD_TOP).into(),
            insets.top.into(),
            NoTrap,
        );
        vm.heap_mut().set_value(
            widgets,
            id!(SAFE_INSET_PAD_BOTTOM).into(),
            insets.bottom.into(),
            NoTrap,
        );
        vm.heap_mut().set_value(
            widgets,
            id!(SAFE_INSET_PAD_LEFT).into(),
            insets.left.into(),
            NoTrap,
        );
        vm.heap_mut().set_value(
            widgets,
            id!(SAFE_INSET_PAD_RIGHT).into(),
            insets.right.into(),
            NoTrap,
        );
    }

    pub fn xr_capabilities(&self) -> &XrCapabilities {
        &self.xr_capabilities
    }

    pub fn xr_tsdf(&self) -> crate::xr_tsdf::XrTsdfStore {
        crate::xr_tsdf::xr_tsdf_store()
    }

    pub fn xr_render_scale(&self) -> Option<f64> {
        <Self as CxOsApi>::xr_render_scale(self)
    }

    pub fn xr_gpu_frame_time_ms(&self) -> Option<f64> {
        <Self as CxOsApi>::xr_gpu_frame_time_ms(self)
    }

    pub fn xr_frame_cpu_time_ms(&self) -> Option<f64> {
        <Self as CxOsApi>::xr_frame_cpu_time_ms(self)
    }

    pub fn xr_render_cpu_time_ms(&self) -> Option<f64> {
        <Self as CxOsApi>::xr_render_cpu_time_ms(self)
    }

    pub fn xr_depth_readback_cpu_time_ms(&self) -> Option<f64> {
        <Self as CxOsApi>::xr_depth_readback_cpu_time_ms(self)
    }

    pub fn xr_frame_cpu_breakdown(&self) -> Option<XrFrameCpuBreakdown> {
        <Self as CxOsApi>::xr_frame_cpu_breakdown(self)
    }

    pub fn xr_display_refresh_rate_hz(&self) -> Option<f64> {
        <Self as CxOsApi>::xr_display_refresh_rate_hz(self)
    }

    pub fn xr_effective_frame_rate_hz(&self) -> Option<f64> {
        <Self as CxOsApi>::xr_effective_frame_rate_hz(self)
    }

    pub fn geometry_pool_slot_count(&self) -> usize {
        self.geometries.0.slot_count()
    }

    pub fn geometry_pool_live_count(&self) -> usize {
        self.geometries.0.live_count()
    }

    pub fn draw_list_pool_slot_count(&self) -> usize {
        self.draw_lists.0.slot_count()
    }

    pub fn draw_list_pool_live_count(&self) -> usize {
        self.draw_lists.0.live_count()
    }

    pub fn texture_pool_slot_count(&self) -> usize {
        self.textures.0.slot_count()
    }

    pub fn texture_pool_live_count(&self) -> usize {
        self.textures.0.live_count()
    }

    /// Apply to the calling thread and read the OS setting back. `Applied`
    /// confirms the requested class/nice value, not a scheduling guarantee.
    pub fn set_thread_priority(priority: CxThreadPriority) -> crate::thread::PriorityStatus {
        use crate::thread::PriorityStatus;
        #[cfg(target_vendor = "apple")]
        {
            unsafe extern "C" {
                fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;
                fn pthread_self() -> *mut std::ffi::c_void;
                fn pthread_get_qos_class_np(
                    thread: *mut std::ffi::c_void,
                    class: *mut u32,
                    relative: *mut i32,
                ) -> i32;
            }
            let (class, relative) = match priority {
                CxThreadPriority::UserInteractive => (0x21, 0),
                CxThreadPriority::UserInitiated => (0x19, 0),
                CxThreadPriority::Normal => (0x15, 0),
                CxThreadPriority::Utility => (0x11, 0),
                CxThreadPriority::Background => (0x09, 0),
                CxThreadPriority::Idle => (0x09, -15),
            };
            let mut actual_class = 0;
            let mut actual_relative = 0;
            let applied = unsafe {
                pthread_set_qos_class_self_np(class, relative) == 0
                    && pthread_get_qos_class_np(
                        pthread_self(),
                        &mut actual_class,
                        &mut actual_relative,
                    ) == 0
                    && actual_class == class
                    && actual_relative == relative
            };
            if applied {
                PriorityStatus::Applied
            } else {
                PriorityStatus::Failed
            }
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            unsafe extern "C" {
                fn setpriority(which: i32, who: u32, priority: i32) -> i32;
                fn getpriority(which: i32, who: u32) -> i32;
            }
            // Linux PRIO_PROCESS, who=0 addresses the current task/thread,
            // not the whole process. Keep SCHED_OTHER; no realtime privilege.
            // Interactive uses nice 0 (unprivileged), light 1 and utility 5.
            let nice = match priority {
                CxThreadPriority::Normal | CxThreadPriority::UserInteractive => 0,
                CxThreadPriority::UserInitiated => 1,
                CxThreadPriority::Utility => 5,
                CxThreadPriority::Background => 10,
                CxThreadPriority::Idle => 15,
            };
            let applied = unsafe { setpriority(0, 0, nice) == 0 && getpriority(0, 0) == nice };
            if applied {
                PriorityStatus::Applied
            } else {
                PriorityStatus::Failed
            }
        }
        #[cfg(target_os = "windows")]
        {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetCurrentThread() -> *mut std::ffi::c_void;
                fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
                fn GetThreadPriority(thread: *mut std::ffi::c_void) -> i32;
            }
            let value = match priority {
                CxThreadPriority::UserInteractive => 2, // HIGHEST, not realtime
                CxThreadPriority::UserInitiated => 1,   // ABOVE_NORMAL
                CxThreadPriority::Normal => 0,
                CxThreadPriority::Utility => -1,
                CxThreadPriority::Background => -2,
                CxThreadPriority::Idle => -15,
            };
            let applied = unsafe {
                let thread = GetCurrentThread();
                SetThreadPriority(thread, value) != 0 && GetThreadPriority(thread) == value
            };
            if applied {
                PriorityStatus::Applied
            } else {
                PriorityStatus::Failed
            }
        }
        #[cfg(not(any(
            target_vendor = "apple",
            target_os = "linux",
            target_os = "android",
            target_os = "windows"
        )))]
        {
            let _ = priority;
            PriorityStatus::BestEffortUnsupported
        }
    }

    pub fn get_ref(&self) -> CxRef {
        CxRef(self.self_ref.clone().unwrap())
    }

    /// Supplies a dependency's bytes by its dependency path (as a package
    /// names it, e.g. `makepad_widgets/resources/Inter.ttf`), so loads of it
    /// are answered from memory instead of the file system or the network
    /// (an app that carries its resources in its own data, such as a
    /// self-contained web build). Takes effect for loads that start after.
    pub fn provide_dependency(&mut self, path: &str, data: Rc<Vec<u8>>) {
        self.dependencies.insert(path.to_string(), crate::cx::CxDependency { data: Some(Ok(data)) });
    }

    pub fn take_dependency(&mut self, path: &str) -> Result<Rc<Vec<u8>>, String> {
        if let Some(data) = self.dependencies.get_mut(path) {
            if let Some(data) = data.data.take() {
                return match data {
                    Ok(data) => Ok(data),
                    Err(s) => Err(s.clone()),
                };
            }
        }

        #[cfg(target_os = "android")]
        if let Some(data) = crate::os::hal::read_asset(path) {
            return Ok(Rc::new(data));
        }

        Err(format!("Dependency not loaded {}", path))
    }

    pub fn get_dependency(&self, path: &str) -> Result<Rc<Vec<u8>>, String> {
        if let Some(data) = self.dependencies.get(path) {
            if let Some(data) = &data.data {
                return match data {
                    Ok(data) => Ok(data.clone()),
                    Err(s) => Err(s.clone()),
                };
            }
        }

        #[cfg(target_os = "android")]
        if let Some(data) = crate::os::hal::read_asset(path) {
            return Ok(Rc::new(data));
        }

        Err(format!("Dependency not loaded {}", path))
    }

    /// Get loaded resource data by the handle's owning heap and value
    /// (`ScriptHandleRef::heap_key` / `as_handle`).
    #[track_caller]
    pub fn get_resource(&self, heap_key: usize, handle: ScriptHandle) -> Option<Rc<Vec<u8>>> {
        // A collect run records every resource the app reads (its assets).
        if let Some(scan) = self.collect_scan() {
            let resources = self.script_data.resources.resources.borrow();
            if let Some(res) = resources.iter().find(|res| res.has_handle(heap_key, handle)) {
                scan.need_asset(res.dependency_path.as_deref().unwrap_or(&res.abs_path));
            }
        }
        if let Some(data) = self.script_data.resources.get_data(heap_key, handle) {
            return Some(data);
        }

        // On web, resources are also available through the dependency table that
        // arrives during ToWasmInit. If a script resource hasn't been promoted to
        // Loaded yet, allow direct dependency lookup as a synchronous fallback.
        if self.os_type().is_web() {
            let resources = self.script_data.resources.resources.borrow();
            if let Some(res) = resources.iter().find(|res| res.has_handle(heap_key, handle)) {
                if let Some(dep_path) = res.dependency_path.as_deref() {
                    if let Ok(data) = self.get_dependency(dep_path) {
                        return Some(data);
                    }
                }
            }
        }

        None
    }

    /// Get the absolute path registered for a script resource handle of
    /// the given heap.
    pub fn get_resource_abs_path(&self, heap_key: usize, handle: ScriptHandle) -> Option<String> {
        let resources = self.script_data.resources.resources.borrow();
        resources
            .iter()
            .find(|res| res.has_handle(heap_key, handle))
            .map(|res| res.abs_path.clone())
    }

    /// Get resource data intended for font parsing.
    ///
    /// This maps local files and stored Android assets, then falls back to
    /// already-loaded resource bytes (required for wasm/network-backed assets).
    pub fn get_resource_font_bytes(&mut self, heap_key: usize, handle: ScriptHandle) -> Option<SharedBytes> {
        let path = self.get_resource_abs_path(heap_key, handle)?;
        self.get_resource_font_bytes_by_path(&path)
    }

    /// Font identity captured in its owning script heap. A local handle alone
    /// cannot identify a resource once a draw object leaves that heap.
    pub fn get_resource_font_bytes_by_path(&self, path: &str) -> Option<SharedBytes> {
        if let Ok(bytes) = SharedBytes::from_file_mmap_or_read(path) {
            return Some(bytes);
        }
        #[cfg(target_os = "android")]
        if let Some(bytes) = self.get_resource_font_asset_bytes_by_path(path) {
            return Some(bytes);
        }
        let resources = self.script_data.resources.resources.borrow();
        let res = resources.iter().find(|res| res.abs_path == path)?;
        if let crate::script::res::CxScriptResourceData::Loaded(data) = &res.data {
            return Some(SharedBytes::from_owned(data.clone()));
        }
        res.dependency_path
            .as_deref()
            .and_then(|path| self.get_dependency(path).ok())
            .map(SharedBytes::from_owned)
    }

    /// Maps an APK font by its resource or dependency path, reusing the mapping across
    /// family loads. Cache actual asset paths so package-root and resource changes can resolve anew.
    #[cfg(target_os = "android")]
    pub fn get_resource_font_asset_bytes_by_path(&self, path: &str) -> Option<SharedBytes> {
        let map_asset = |path: &str| {
            let mut assets = self.hal.mapped_font_assets.borrow_mut();
            if let Some(bytes) = assets.get(path) {
                return bytes.clone();
            }
            let bytes = crate::os::hal::map_asset(path);
            assets.insert(path.to_string(), bytes.clone());
            bytes
        };
        let map_dependency = |path: &str| {
            map_asset(path).or_else(|| {
                let package_root = self.package_root.as_deref()?;
                let root_prefix = format!("{}/", package_root);
                if path.starts_with(&root_prefix) {
                    return None;
                }
                map_asset(&format!("{}{}", root_prefix, path))
            })
        };
        if let Some(bytes) = map_dependency(path) {
            return Some(bytes);
        }
        let resources = self.script_data.resources.resources.borrow();
        let res = resources.iter().find(|res| res.abs_path == path)?;
        map_dependency(res.dependency_path.as_deref()?)
    }

    /// The script resource at `path` can never be read in this process: no
    /// resource is registered under it, or its load already failed (a font
    /// left out of the build's font set, an asset missing from the package).
    /// A resource still loading, or not yet loaded, is not unavailable.
    pub fn script_resource_generation(&self) -> u64 {
        self.script_data.resources.generation.get()
    }

    pub fn script_resource_unavailable(&self, path: &str) -> bool {
        let resources = self.script_data.resources.resources.borrow();
        match resources.iter().find(|res| res.abs_path == path) {
            None => true,
            Some(res) => matches!(res.data, crate::script::res::CxScriptResourceData::Error(_)),
        }
    }

    pub fn null_texture(&self) -> Texture {
        self.null_texture.clone()
    }
    pub fn null_cube_texture(&self) -> Texture {
        self.null_cube_texture.clone()
    }
    pub fn redraw_id(&self) -> u64 {
        self.redraw_id
    }

    /// The drawing being recorded: one per draw event, however many draw
    /// contexts it makes (a second window's, an offscreen bake's), and one per
    /// draw context made outside a draw event. A recorder that keeps state
    /// between frames counts frames by it, not by `redraw_id`.
    pub fn drawing_id(&self) -> u64 {
        self.drawing_id
    }

    /// A draw context begins (`CxDraw::new`): a new `redraw_id` for the lists
    /// it records, and, outside a draw event, a drawing of its own.
    pub fn begin_draw_context(&mut self) {
        if !self.in_draw_event {
            self.drawing_id += 1;
            self.drawing_start = self.redraw_id;
        }
        self.redraw_id += 1;
    }

    pub fn os_type(&self) -> &OsType {
        &self.os_type
    }

    /// Linux X11: hide the window manager's decorations (the system title
    /// bar) so the app draws its own chrome. Wayland picks client-side
    /// decorations on its own. Call from `Startup`, before the window is
    /// created; windows created afterwards follow the setting.
    pub fn set_linux_custom_window_chrome(&mut self, enable: bool) {
        if let OsType::LinuxWindow(params) = &mut self.os_type {
            params.custom_window_chrome = enable;
        }
    }

    /// The GPU API selected by the active backend.
    pub fn gpu_backend(&self) -> GpuBackend {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match self.hal.info.map(|info| info.backend) {
                Some(makepad_hal::backend_id::METAL) => GpuBackend::Metal,
                Some(makepad_hal::backend_id::VULKAN) => GpuBackend::Vulkan,
                Some(makepad_hal::backend_id::GL) => GpuBackend::OpenGl,
                None if cfg!(target_vendor = "apple") => GpuBackend::Metal,
                None if cfg!(target_os = "linux") => GpuBackend::Vulkan,
                Some(makepad_hal::backend_id::D3D11) => GpuBackend::Direct3d11,
                _ => GpuBackend::Headless,
            }
        }
        #[cfg(target_arch = "wasm32")]
        { GpuBackend::WebGl }
    }

    /// Returns the app's writable data directory path.
    ///
    /// On Android, this is the directory returned by Activity's getFilesDir().
    /// On iOS, this is the Application Support directory.
    /// Returns None on unsupported platforms (e.g. wasm).
    ///
    /// Note that this path is not guaranteed to exist (it doesn't by default on iOS simulators),
    /// so you might need to create it.
    pub fn get_data_dir(&self) -> Option<String> {
        self.os_type.get_data_dir()
    }

    pub fn in_makepad_studio(&self) -> bool {
        self.in_makepad_studio
    }

    pub fn cpu_cores(&self) -> usize {
        self.cpu_cores
    }
    pub fn gpu_info(&self) -> &GpuInfo {
        &self.gpu_info
    }

    pub fn update_macos_menu(&mut self, menu: MacosMenu) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops.push_back(CxOsOp::UpdateMacosMenu(menu));
    }

    pub fn xr_start_presenting(&mut self) {
        self.platform_ops.push_back(CxOsOp::XrStartPresenting);
    }

    pub fn xr_set_render_scale(&mut self, scale: f32) {
        self.platform_ops.push_back(CxOsOp::XrSetRenderScale(scale));
    }

    pub fn xr_advertise_anchor(&mut self, anchor: XrAnchor) {
        self.platform_ops
            .push_back(CxOsOp::XrAdvertiseAnchor(anchor));
    }

    pub fn xr_set_local_anchor(&mut self, anchor: XrAnchor) {
        self.platform_ops
            .push_back(CxOsOp::XrSetLocalAnchor(anchor));
    }

    pub fn xr_set_local_floor(&mut self, floor_y: f32) {
        self.platform_ops
            .push_back(CxOsOp::XrSetLocalFloor(floor_y));
    }

    pub fn xr_discover_anchor(&mut self, id: u8) {
        self.platform_ops.push_back(CxOsOp::XrDiscoverAnchor(id));
    }

    #[cfg_attr(target_os = "linux", track_caller)]
    pub fn quit(&mut self) {
        #[cfg(target_os = "linux")]
        if std::env::var_os("MAKEPAD_HAL_LOG").is_some() {
            eprintln!("[hal-linux] Cx::quit queued at {}", std::panic::Location::caller());
        }
        crate::cx::relax_quit_started();
        self.platform_ops.push_back(CxOsOp::Quit);
    }

    pub fn request_quit(&mut self, reason: QuitReason) -> bool {
        #[cfg(target_os = "linux")]
        if std::env::var_os("MAKEPAD_HAL_LOG").is_some() {
            eprintln!("[hal-linux] Cx::request_quit reason={reason:?}");
        }
        let event = Event::QuitRequested(QuitRequestedEvent::new(reason));
        self.call_event_handler(&event);

        let was_handled = match &event {
            Event::QuitRequested(e) => e.handled.get(),
            _ => false,
        };
        if !was_handled {
            self.quit();
        }
        was_handled
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub(crate) fn handle_termination_signal(&mut self) {
        if crate::os::termination_signal::take_requested() {
            self.request_quit(QuitReason::Signal);
        }
    }

    pub fn browser_update_url(&mut self, url: &str, replace: bool) {
        if self.script_data.std.host_io_only() { return; }
        <Self as CxOsApi>::browser_update_url(self, url, replace);
    }

    pub fn browser_history_go(&mut self, delta: i32) {
        if self.script_data.std.host_io_only() { return; }
        <Self as CxOsApi>::browser_history_go(self, delta);
    }

    pub fn system_browser(&mut self, id: impl Into<SystemBrowserId>) -> CxSystemBrowser<'_> {
        CxSystemBrowser {
            cx: self,
            id: id.into(),
        }
    }

    // Determines whether to show your application in the dock when it runs. The default value is true.
    // You can remove the dock icon by setting this value to false.
    /// FPS mouse capture: lock hides and freezes the hardware cursor while
    /// deltas keep flowing as MouseMove events; unlock restores the cursor.
    /// Convention: lock on the game's own click, RELEASE ON ESCAPE.
    pub fn lock_mouse_pointer(&mut self, lock: bool) {
        self.platform_ops.push_back(CxOsOp::LockMousePointer(lock));
    }

    /// Call once per frame while holding the lock — keeps the hardware
    /// cursor pinned even when the OS quietly drops the disassociation.
    pub fn repin_mouse_pointer(&mut self) {
        self.platform_ops.push_back(CxOsOp::RepinMousePointer);
    }

    /// Pin the CURRENT mouse capture for value scrubbing: the hardware
    /// cursor hides AT its position and detaches so deltas keep flowing
    /// with infinite range; the drag owner keeps receiving FingerMove
    /// through its ordinary capture, and nothing else in the window sees
    /// the pointer (no hover, no new captures). Engage only when the drag
    /// actually starts (the 3px threshold crossing), never on the initial
    /// press. Release is AUTOMATIC: the pin rides on the capture, and the
    /// hardware button-up releases both (plus focus-loss and the platform
    /// layer's own unconditional release) — the cursor restores at the
    /// press point. Call `unpin_pointer_capture` only to cancel EARLY
    /// (Escape / right-click) while the button is still held.
    pub fn pin_pointer_capture(&mut self) {
        // A drag injected through `--remote` (an automated, often hidden
        // instance) never pins: the pin hides and detaches the machine's
        // real cursor, which belongs to whoever sits at it. The injected
        // moves carry their own positions and need no pin.
        #[cfg(not(target_arch = "wasm32"))]
        if self.remote_activity.remote_input.get() {
            return;
        }
        if self.fingers.pin_mouse_capture() {
            self.platform_ops.push_back(CxOsOp::PinMousePointer(true));
        }
    }

    /// Cancel a scrub pin while the button is still held (Escape /
    /// right-click cancel): clears the capture's pin flag and restores the
    /// cursor at the press point.
    /// The repaint counter: one step per presented frame. Two reads apart
    /// in time say whether the app paints on its own.
    pub fn repaint_id(&self) -> u64 {
        self.repaint_id
    }

    pub fn unpin_pointer_capture(&mut self) {
        if self.fingers.unpin_captures() {
            self.platform_ops.push_back(CxOsOp::PinMousePointer(false));
        }
    }

    pub fn show_in_dock(&mut self, show: bool) {
        self.platform_ops.push_back(CxOsOp::ShowInDock(show));
    }

    /// Controls how the system bars (status bar and navigation bar) icons and
    /// text are tinted, on platforms that support it (currently Android only).
    ///
    /// The default is [`SystemBarAppearance::Auto`], which picks dark or light
    /// system-bar icons automatically from the luminance of the window's
    /// background color (`clear_color`): a light background gets dark icons, a
    /// dark background gets light icons. Pass [`SystemBarAppearance::DarkIcons`]
    /// or [`SystemBarAppearance::LightIcons`] to override that automatic choice.
    ///
    /// The request is resolved and applied by the `Window` widget on its next
    /// event cycle.
    pub fn set_system_bar_appearance(&mut self, appearance: SystemBarAppearance) {
        self.display_context.system_bar_appearance = appearance;
    }

    /// Ask the OS to defer its own gestures on these screen edges, so the
    /// FIRST swipe from such an edge reaches the app (an app-owned home
    /// gesture, an edge-dragged drawer); the OS then shows its indicator and
    /// takes the next one — the immersive/game behaviour. [`ScreenEdges::NONE`]
    /// gives the edges back. One call on every platform: honored on iOS
    /// (`preferredScreenEdgesDeferringSystemGestures`), nothing where the OS
    /// has no such gesture.
    pub fn defer_system_gestures(&mut self, edges: ScreenEdges) {
        if !matches!(self.os_type(), OsType::Ios(_)) {
            return;
        }
        self.platform_ops
            .retain(|op| !matches!(op, CxOsOp::DeferSystemGestures(_)));
        self.platform_ops.push_back(CxOsOp::DeferSystemGestures(edges));
    }
    /// Locks the screen to portrait or landscape, or with
    /// [`ScreenOrientation::Auto`] releases the lock. Honored on Android and
    /// iOS (on iOS 16+ the screen turns at once, before that at the next
    /// device rotation); nothing elsewhere. A lock the app's manifest or
    /// Info.plist does not allow is the app's own error. The latest request
    /// of a frame wins.
    pub fn set_screen_orientation(&mut self, orientation: ScreenOrientation) {
        if !matches!(self.os_type(), OsType::Android(_) | OsType::Ios(_)) {
            return;
        }
        self.platform_ops
            .retain(|op| !matches!(op, CxOsOp::SetScreenOrientation(_)));
        self.platform_ops
            .push_back(CxOsOp::SetScreenOrientation(orientation));
    }

    /// Keeps the display awake (no dimming, no sleep) while `on`, e.g. while
    /// a video plays. Honored on Android and iOS; nothing elsewhere. The
    /// latest request of a frame wins.
    pub fn set_keep_screen_on(&mut self, on: bool) {
        if !matches!(self.os_type(), OsType::Android(_) | OsType::Ios(_)) {
            return;
        }
        self.platform_ops
            .retain(|op| !matches!(op, CxOsOp::SetKeepScreenOn(_)));
        self.platform_ops.push_back(CxOsOp::SetKeepScreenOn(on));
    }

    pub fn push_unique_platform_op(&mut self, op: CxOsOp) {
        if self.platform_ops.iter().find(|o| **o == op).is_none() {
            self.platform_ops.push_back(op);
        }
    }

    pub fn show_text_ime(&mut self, area: Area, pos: Vec2d) {
        // No line metrics from this entry point: a zero-height rect anchors the
        // IME at the bare point (same behavior as before the rect change).
        self.show_text_ime_with_config(
            area,
            Rect {
                pos,
                size: Vec2d::default(),
            },
            TextInputConfig::default(),
        );
    }

    pub fn show_text_ime_with_config(
        &mut self,
        area: Area,
        cursor_rect: Rect,
        config: TextInputConfig,
    ) {
        if !self.keyboard.text_ime_dismissed {
            self.ime_area = area;
            let rect = area.rect(self);
            self.publish_hosted_ime(crate::ime::HostedImeState {
                visible: !config.is_read_only
                    && config.soft_keyboard.input_mode != crate::ime::InputMode::None,
                input_mode: config.soft_keyboard.input_mode,
                return_key: config.soft_keyboard.return_key_type,
                multiline: config.is_multiline,
                x: rect.pos.x,
                y: rect.pos.y,
                width: rect.size.x,
                height: rect.size.y,
            });
            self.platform_ops
                .push_back(CxOsOp::ShowTextIME(area, cursor_rect, config));
        }
    }

    pub fn sync_ime_state(
        &mut self,
        text: String,
        selection: Range<CharOffset>,
        composition: Option<Range<CharOffset>>,
    ) {
        self.platform_ops.push_back(CxOsOp::SyncImeState {
            text,
            selection,
            composition,
        });
    }

    pub fn hide_text_ime(&mut self) {
        self.publish_hosted_ime(crate::ime::HostedImeState::default());
        self.keyboard.reset_text_ime_dismissed();
        self.platform_ops.push_back(CxOsOp::HideTextIME);
    }

    /// Whether keys go to a text field now: the widget with the key focus
    /// is the one that raised the text IME (every text input does on
    /// focus). An app's bare-key shortcuts (Space, arrows, letters) stand
    /// down while it is true.
    pub fn text_input_has_focus(&self) -> bool {
        let focus = self.key_focus();
        !focus.is_empty() && focus == self.ime_area
    }

    pub fn text_ime_was_dismissed(&mut self) {
        self.publish_hosted_ime(crate::ime::HostedImeState::default());
        self.keyboard.set_text_ime_dismissed();
        // A focused TextInput re-pushes ShowTextIME on every draw, so a show op
        // queued by a frame drawn just before this dismissal landed can still be
        // in platform_ops. If it drains after the backend forgets its last-shown
        // config (e.g. Android's ResizeTextIME-closed clearing last_ime_config),
        // it re-opens the keyboard the user just closed. Drop the stale shows.
        self.platform_ops
            .retain(|op| !matches!(op, CxOsOp::ShowTextIME(..)));
        self.platform_ops.push_back(CxOsOp::HideTextIME);
    }

    pub fn hosted_ime_state(&self) -> crate::ime::HostedImeState {
        self.get_global_ref::<crate::ime::HostedImeState>()
            .cloned()
            .unwrap_or_default()
    }

    fn publish_hosted_ime(&mut self, state: crate::ime::HostedImeState) {
        if self.get_global_ref::<crate::ime::HostedImeState>() == Some(&state) {
            return;
        }
        if self.in_makepad_studio {
            Self::send_studio_message(crate::studio::AppToStudio::Custom(state.to_json()));
        }
        *self.global::<crate::ime::HostedImeState>() = state;
    }

    /// Set or clear a window's `dpi_override` at runtime.
    ///
    /// This rewrites the stored `WindowGeom` from the previous effective DPI to
    /// the new effective DPI, including every in-window metric that must remain
    /// physically fixed: inner/outer size, safe-area insets, and native chrome
    /// button bounds. The resulting synthetic `WindowGeomChange` is queued so
    /// this can be called from inside normal event/action handlers.
    pub fn set_window_dpi_override(&mut self, window_id: WindowId, dpi_override: Option<f64>) {
        let dpi_override = dpi_override.and_then(CxWindow::valid_dpi_factor);
        if matches!(self.os_type(), OsType::Macos) {
            self.platform_ops
                .retain(|queued| !matches!(queued, CxOsOp::SetWindowDpiOverride(id, _) if *id == window_id));
            self.platform_ops.push_back(CxOsOp::SetWindowDpiOverride(window_id, dpi_override));
        }
        let current_dpi = self.windows[window_id].effective_dpi_factor();
        let target_dpi = dpi_override.unwrap_or_else(|| self.windows[window_id].native_dpi_factor());
        #[cfg(target_os = "linux")]
        {
            self.windows[window_id].dpi_override = dpi_override;
            self.hal_sync_direct_scale();
        }
        let window = &mut self.windows[window_id];

        if (target_dpi - current_dpi).abs() < f64::EPSILON {
            window.dpi_override = dpi_override;
            window.window_geom.dpi_factor = target_dpi;
            return;
        }

        let old_geom = window.window_geom.clone();
        let scale = current_dpi / target_dpi;
        window.dpi_override = dpi_override;
        window.window_geom = window.scale_window_geom(old_geom.clone(), scale, target_dpi);
        let new_geom = window.window_geom.clone();

        self.pending_window_geom_changes
            .push(crate::event::WindowGeomChangeEvent {
                window_id,
                old_geom,
                new_geom,
            });
        self.redraw_all();
    }

    /// Shows the native clipboard actions menu (Copy/Paste/Cut/Select All).
    ///
    /// Displays a platform-specific floating menu with text editing actions. The menu items
    /// are enabled/disabled based on the current selection state:
    /// - Copy/Cut: Only shown when `has_selection` is true
    /// - Paste: Only shown when clipboard has content
    /// - Select All: Always shown
    ///
    /// # Parameters
    /// * `has_selection` - Whether text is currently selected (enables Copy/Cut actions)
    /// * `rect` - Selection bounding box in Makepad layout points (for menu positioning)
    /// * `keyboard_shift` - Vertical offset caused by virtual keyboard (in Makepad layout points)
    ///
    /// # Platform Support
    /// - Android: Uses ActionMode with floating toolbar
    /// - iOS: TODO - Will use UIMenuController
    /// - Other platforms: No-op
    ///
    /// # Note
    /// The actual clipboard operations (copy/cut/paste) are performed by querying
    /// the text selection from Rust directly. The `has_selection` parameter is only
    /// used to determine which menu items to show, not for the operations themselves.
    pub fn show_clipboard_actions(&mut self, has_selection: bool, rect: Rect, keyboard_shift: f64) {
        self.platform_ops.push_back(CxOsOp::ShowClipboardActions {
            has_selection,
            rect,
            keyboard_shift,
        });
    }

    /// Hides the clipboard actions menu
    pub fn hide_clipboard_actions(&mut self) {
        self.platform_ops.push_back(CxOsOp::HideClipboardActions);
    }

    /// Copies the given string to the clipboard.
    ///
    /// Due to lack of platform clipboard support, it does not work on Web or tvOS.
    /// A system haptic (Android: the HAL's Haptic command; a no-op elsewhere).
    pub fn haptic_feedback(&mut self, kind: HapticFeedback) {
        #[cfg(target_os = "android")]
        self.platform_ops.push_back(CxOsOp::HapticFeedback(kind));
        #[cfg(not(target_os = "android"))]
        let _ = kind;
    }

    /// Brings another running app, by process id, to the front (a launcher showing the app it
    /// started). macOS and Windows; false where unsupported or refused.
    #[cfg(not(target_os = "linux"))]
    pub fn activate_process(&mut self, pid: u32) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        return makepad_hal::activate_process(pid);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = pid;
            false
        }
    }

    pub fn copy_to_clipboard(&mut self, content: &str) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops
            .push_back(CxOsOp::CopyToClipboard(content.to_owned()));
    }

    /// Open the OS share sheet (Android `ACTION_SEND` chooser) with `content`.
    /// No-op on platforms whose backend doesn't handle `CxOsOp::ShareText`.
    pub fn share_text(&mut self, content: &str) {
        self.platform_ops
            .push_back(CxOsOp::ShareText(content.to_owned()));
    }

    /// Sets the primary selection (Linux middle-click paste).
    /// No-op on non-Linux platforms.
    pub fn set_primary_selection(&mut self, content: &str) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops
            .push_back(CxOsOp::SetPrimarySelection(content.to_owned()));
    }

    /// Forward an accessibility tree update to the platform adapter.
    ///
    /// The `update` is a type-erased `accesskit::TreeUpdate`. Platform backends
    /// downcast it when an accessibility adapter is active.
    pub fn update_accessibility_tree(&mut self, update: Box<dyn std::any::Any + Send>) {
        self.platform_ops
            .push_back(CxOsOp::AccessibilityUpdate(AccessibilityUpdatePayload(
                update,
            )));
    }

    /// Show native selection handles at the given start and end positions (mobile).
    pub fn show_selection_handles(&mut self, start: Vec2d, end: Vec2d) {
        self.platform_ops
            .push_back(CxOsOp::ShowSelectionHandles { start, end });
    }

    /// Update positions of visible selection handles (mobile).
    pub fn update_selection_handles(&mut self, start: Vec2d, end: Vec2d) {
        self.platform_ops
            .push_back(CxOsOp::UpdateSelectionHandles { start, end });
    }

    /// Hide selection handles (mobile).
    pub fn hide_selection_handles(&mut self) {
        self.platform_ops.push_back(CxOsOp::HideSelectionHandles);
    }

    /// Start a drag of `items` inside the app. Items that are not files
    /// (strings, and anything with an `internal_id`) are the app's own
    /// data, so they are dragged by the platform-independent path on every
    /// backend: the pointer's moves become `Event::Drag`, its release
    /// `Event::Drop` then `Event::DragEnd`. Files alone go to the OS drag
    /// where a backend has one, since a file may leave the app.
    ///
    /// Android and iOS are the exception: a finger sends touch updates, not
    /// mouse moves and ups, so the shared path would never see the drag move
    /// or end, and the next drag would find the last one still running. Their
    /// backends drive every drag from the touch stream, cancel included.
    pub fn start_dragging(&mut self, items: Vec<DragItem>) {
        if self.script_data.std.host_io_only() { return; }
        let files_only = items
            .iter()
            .all(|item| matches!(item, DragItem::FilePath { internal_id: None, .. }));
        let touch_backend = cfg!(any(target_os = "android", target_os = "ios"));
        let os_drag = cfg!(not(any(target_arch = "wasm32", target_os = "linux", test)))
            && (files_only || touch_backend);
        if !os_drag {
            self.drag_drop.start_internal_drag(items);
            return;
        }
        #[cfg(not(any(target_arch = "wasm32", target_os = "linux", test)))]
        {
            self.platform_ops.iter().for_each(|p| {
                if let CxOsOp::StartDragging { .. } = p {
                    panic!("start drag twice");
                }
            });
            self.platform_ops.push_back(CxOsOp::StartDragging(items));
        }
    }

    /// Starts a native drag-and-drop session that can leave the Makepad app.
    ///
    /// `FilePath` items must contain absolute paths to existing regular files
    /// and must not carry an `internal_id`. The source window is explicit so
    /// a multi-window application never starts the session from a stale Cocoa
    /// event or the wrong native surface. Unsupported item/platform
    /// combinations are rejected by the platform adapter. Every accepted or
    /// rejected request completes with `Event::DragEnd`, allowing callers to
    /// release gesture state without platform-specific timeouts.
    pub fn start_external_dragging(&mut self, window_id: WindowId, items: Vec<DragItem>) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops.iter().for_each(|op| {
            if matches!(op, CxOsOp::StartExternalDragging { .. }) {
                panic!("start external drag twice");
            }
        });
        self.platform_ops
            .push_back(CxOsOp::StartExternalDragging { window_id, items });
    }

    pub fn set_cursor(&mut self, cursor: MouseCursor) {
        self.mouse_cursor = cursor;
        // down cursor overrides the hover cursor
        if let Some(p) = self.platform_ops.iter_mut().find(|p| match p {
            CxOsOp::SetCursor(_) => true,
            _ => false,
        }) {
            *p = CxOsOp::SetCursor(cursor)
        } else {
            self.platform_ops.push_back(CxOsOp::SetCursor(cursor))
        }
    }

    pub fn mouse_cursor(&self) -> MouseCursor {
        self.mouse_cursor
    }

    pub fn sweep_lock(&mut self, value: Area) {
        self.fingers.sweep_lock(value);
    }

    /// Takes a scoped sweep lock above existing owners, unless this area already owns one.
    pub fn acquire_sweep_lock(&mut self, value: Area) -> Option<crate::event::SweepLock> {
        self.fingers.acquire_sweep_lock(value)
    }

    /// Hand the finger currently captured by `from` over to `to` (with sweep area
    /// `to_sweep`), so a drag begun on one widget can continue on another — e.g. a
    /// long-pressed drawer app handing its touch to the home pager for placement.
    /// Returns true if a live capture on `from` was found and handed over; false if
    /// the finger was already released, so callers can avoid starting a dead drag.
    pub fn switch_finger_capture(&mut self, from: Area, to: Area, to_sweep: Area) -> bool {
        self.fingers.switch_capture_area(from, to, to_sweep)
    }

    /// Hand a finger grabbed by an interactive child up to `over`, a container
    /// that already co-captures it via `capture_overload` — so `over` (e.g. the
    /// home pager) can drive a pan/drag even when the press started on a button
    /// inside one of its children. Returns true if a child capture was dropped.
    pub fn promote_finger_capture_over(&mut self, over: Area) -> bool {
        self.fingers.promote_capture_over(over)
    }

    /// `area` takes the gesture of the finger `digit_id` (a scroller that
    /// starts scrolling): it becomes the finger's one owner and every other
    /// capture of it is cancelled. False when the gesture is not `area`'s
    /// to take (see `CxFingers::claim_gesture`) — then it must stand down.
    /// On success the owner dispatches `Event::FingerCancel` to its
    /// children so the presses it took end at once.
    pub fn claim_finger_gesture(&mut self, digit_id: crate::event::DigitId, area: Area) -> bool {
        self.fingers.claim_gesture(digit_id, area)
    }

    /// Call on `Hit::FingerDown` when your widget gives this finger's drag its own meaning
    /// (text selection, a slider), so containers won't turn the drag into their own gesture.
    pub fn claim_finger_drag(&mut self, digit_id: DigitId) {
        self.fingers.claim_drag(digit_id);
    }

    /// Drops every widget's capture of the touch with the given uid, so the rest of that touch hits nothing.
    /// Call it once those widgets saw the touch get canceled.
    pub fn release_touch_captures(&mut self, uid: u64) {
        self.fingers.release_digit(live_id_num!(touch, uid).into());
    }

    pub fn sweep_unlock(&mut self, value: Area) {
        self.fingers.sweep_unlock(value);
    }

    /// The area holding the sweep lock right now, if any.
    ///
    /// A popover that takes the pointer while it is open (a drop-down, a
    /// radial menu, a drawer) reads this to tell its own grab from one an
    /// overlay above it took, so it releases only what it locked itself.
    pub fn sweep_lock_area(&self) -> Option<Area> {
        self.fingers.sweep_lock_area()
    }

    /// Returns whether scrolling is currently allowed within the given `area`.
    pub fn is_scrolling_allowed_within(&mut self, area: &Area) -> bool {
        let Some(scrollable_area) = self.fingers.blocked_scrolling_exception_area() else {
            return true;
        };
        area.rect(self).is_inside_of(scrollable_area.rect(self))
    }

    /// Blocks scrolling events/hits in the app *EXCEPT* for within the given `scrollable_area`.
    ///
    /// Blocks nest: a second owner (a modal over a modal) takes over until it
    /// releases its own block, and the first owner's block then applies again.
    /// The same owner may call this every draw and keeps its single entry.
    ///
    /// ***NOTE***: this must be re-invoked every time the area changes, which is upon every draw pass.
    ///
    /// If you want to block scrolling everywhere, pass in `Area::Empty`.
    pub fn block_scrolling_except_within(&mut self, scrollable_area: Area) {
        self.fingers
            .block_scrolling_within_area(Some(scrollable_area));
    }

    /// Releases the innermost scroll block — the most recent
    /// [`Cx::block_scrolling_except_within()`] whose owner has not released
    /// it. With a single owner this restores the default, scrolling anywhere.
    /// An owner that knows its area should call
    /// [`Cx::unblock_scrolling_within_area()`] instead, so it never pops a
    /// block another owner pushed over it.
    pub fn unblock_scrolling(&mut self) {
        self.fingers.block_scrolling_within_area(None);
    }

    /// Releases the scroll block owned by `scrollable_area`; a block held by
    /// another owner is left alone.
    ///
    /// The owner-scoped counterpart of [`Cx::unblock_scrolling()`], for a
    /// panel that closes without knowing whether something else has since
    /// blocked scrolling over it.
    pub fn unblock_scrolling_within_area(&mut self, scrollable_area: Area) {
        self.fingers.unblock_scrolling_within_area(scrollable_area);
    }

    #[track_caller]
    pub fn start_timeout(&mut self, delay: f64) -> Timer {
        self.timer_id += 1;
        crate::frame_sources::timer_started(self.timer_id, std::panic::Location::caller());
        if let Some(now) = crate::remote::app_clock::now() {
            self.app_clock.start_timer(self.timer_id, delay, false, now);
            return Timer(self.timer_id);
        }
        self.platform_ops.push_back(CxOsOp::StartTimer {
            timer_id: self.timer_id,
            interval: delay,
            repeats: false,
        });
        Timer(self.timer_id)
    }

    #[track_caller]
    pub fn start_interval(&mut self, interval: f64) -> Timer {
        self.timer_id += 1;
        crate::frame_sources::timer_started(self.timer_id, std::panic::Location::caller());
        if let Some(now) = crate::remote::app_clock::now() {
            self.app_clock.start_timer(self.timer_id, interval, true, now);
            return Timer(self.timer_id);
        }
        self.platform_ops.push_back(CxOsOp::StartTimer {
            timer_id: self.timer_id,
            interval,
            repeats: true,
        });
        Timer(self.timer_id)
    }

    pub fn stop_timer(&mut self, timer: Timer) {
        if crate::remote::app_clock::enabled() {
            self.app_clock.stop_timer(timer.0);
            return;
        }
        if timer.0 != 0 {
            self.platform_ops.push_back(CxOsOp::StopTimer(timer.0));
        }
    }

    pub fn request_permission(&mut self, permission: crate::permission::Permission) -> i32 {
        self.permissions_request_id += 1;
        self.platform_ops.push_back(CxOsOp::RequestPermission {
            request_id: self.permissions_request_id,
            permission,
        });
        self.permissions_request_id
    }

    pub fn check_permission(&mut self, permission: crate::permission::Permission) -> i32 {
        self.permissions_request_id += 1;
        self.platform_ops.push_back(CxOsOp::CheckPermission {
            request_id: self.permissions_request_id,
            permission,
        });
        self.permissions_request_id
    }

    /// Start streaming position fixes from the platform location service
    /// (CoreLocation on macOS/iOS, LocationManager on Android,
    /// `navigator.geolocation` on web). Fixes arrive as
    /// [`Event::LocationUpdate`]; permission prompts are handled by the
    /// platform, failures arrive as [`Event::LocationError`]. Platforms
    /// without a location service log an error and stay silent.
    pub fn start_location_updates(&mut self) {
        self.platform_ops.push_back(CxOsOp::StartLocationUpdates);
    }

    /// Stop streaming position fixes.
    pub fn stop_location_updates(&mut self) {
        self.platform_ops.push_back(CxOsOp::StopLocationUpdates);
    }

    pub fn get_dpi_factor_of(&mut self, area: &Area) -> f64 {
        if let Some(draw_list_id) = area.draw_list_id() {
            let draw_pass_id = self.draw_lists[draw_list_id].draw_pass_id.unwrap();
            return self.get_delegated_dpi_factor(draw_pass_id);
        }
        return 1.0;
    }

    pub fn get_window_id_of(&self, area: &Area) -> Option<WindowId> {
        let draw_list_id = area.draw_list_id()?;
        let draw_pass_id = self.draw_lists[draw_list_id].draw_pass_id?;
        self.get_pass_window_id(draw_pass_id)
    }

    pub fn get_pass_window_id(&self, draw_pass_id: DrawPassId) -> Option<WindowId> {
        let mut pass_id_walk = draw_pass_id;
        for _ in 0..25 {
            match self.passes[pass_id_walk].parent {
                CxDrawPassParent::Window(window_id) => return Some(window_id),
                CxDrawPassParent::DrawPass(next_pass_id) => {
                    pass_id_walk = next_pass_id;
                }
                _ => {
                    break;
                }
            }
        }
        None
    }

    pub fn get_delegated_dpi_factor(&mut self, draw_pass_id: DrawPassId) -> f64 {
        let mut pass_id_walk = draw_pass_id;
        for _ in 0..25 {
            match self.passes[pass_id_walk].parent {
                CxDrawPassParent::Window(window_id) => {
                    if !self.windows[window_id].is_created {
                        return 1.0;
                    }
                    return self.windows[window_id].window_geom.dpi_factor;
                }
                CxDrawPassParent::DrawPass(next_pass_id) => {
                    pass_id_walk = next_pass_id;
                }
                _ => {
                    break;
                }
            }
        }
        1.0
    }

    /// A pass's rect in whole pixels. An area's rect comes from f32 instance data: 1080 px at
    /// 2.4375 reads back as 443.07693 pt = 1080.00003 px, and a ceil with no allowance made the
    /// pass (the WM's wallpaper CachedView) 1081 px wide -- drawn back into a 1080-px window it was
    /// squeezed a pixel and the last column showed the clear colour (the Pixel's white line).
    /// A hundredth of a pixel is rounding, not content.
    pub fn get_pass_rect(&self, draw_pass_id: DrawPassId, dpi: f64) -> Option<Rect> {
        let size = |size: Vec2d| Self::pass_logical_size(size, dpi);
        match self.passes[draw_pass_id].pass_rect {
            Some(CxDrawPassRect::Area(area)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: ((rect.pos * dpi) + 0.01).floor() / dpi,
                    size: size(rect.size),
                })
            }
            Some(CxDrawPassRect::AreaOrigin(area, origin)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: origin,
                    size: size(rect.size),
                })
            }
            /*Some(CxDrawPassRect::ScaledArea(area, scale)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: ((rect.pos * dpi) + 0.01).floor() / dpi,
                    size: scale * ((rect.size * dpi) - 0.01).ceil() / dpi,
                })
            }*/
            Some(CxDrawPassRect::Size(s)) => Some(Rect {
                pos: Vec2d::default(),
                size: size(s),
            }),
            None => None,
        }
    }

    /// A pass's logical size snapped to its whole pixels (`pass_pixel_size` / dpi).
    pub fn pass_logical_size(size: Vec2d, dpi: f64) -> Vec2d {
        let (w, h) = Self::pass_pixel_size(size, dpi);
        dvec2(w as f64 / dpi, h as f64 / dpi)
    }

    /// The pixel size of a texture pass of logical `size` at `dpi`: the ONE rounding of a pass's
    /// size to whole pixels (`get_pass_rect`, the frame's pass size and viewport). A render
    /// target a pass draws into is sized with it too, so the pass is exactly its target: a
    /// target floored where the pass was ceiled was a pixel smaller at fractional scales (1.25,
    /// 1.41, 1.75), and the GPU refused the pass every frame (Scope's Glass look and History
    /// view drew nothing on 149). A hundredth of a pixel is rounding, not content.
    pub fn pass_pixel_size(size: Vec2d, dpi: f64) -> (usize, usize) {
        let px = |v: f64| {
            let p = (v * dpi - 0.01).ceil();
            if p.is_finite() && p > 0.0 { p as usize } else { 0 }
        };
        (px(size.x), px(size.y))
    }

    /// What one texel of a BGRA8 render target costs on this backend: the
    /// HAL and WebGL allocate 4 bytes. Caches that budget render targets charge this.
    pub fn render_target_bytes_per_texel(&self) -> usize {
        4
    }

    /// What one texel of a `DepthD32` attachment costs (4 bytes everywhere).
    pub fn depth_target_bytes_per_texel(&self) -> usize {
        4
    }

    /// Depth attachments have their own storage, independently of color targets.
    pub fn depth_target_rides_with_render_target(&self) -> bool {
        false
    }

    pub fn get_pass_name(&self, draw_pass_id: DrawPassId) -> &str {
        &self.passes[draw_pass_id].debug_name
    }

    pub fn repaint_pass(&mut self, draw_pass_id: DrawPassId) {
        let cxpass = &mut self.passes[draw_pass_id];
        cxpass.paint_dirty = true;
        cxpass.repaint_requested = true;
    }

    /// An external grab/input-frame request is work even for an idle, hidden
    /// window. Repaint its pass tree without invalidating tweaked draw buffers.
    /// Standalone macOS first records any already-pending Draw, then submits
    /// this window before later input, without advancing NextFrame. Other
    /// backends service this work on their ordinary next-render path.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn request_remote_window_present(&mut self, window_id: WindowId) {
        if let Some(pass) = self.windows[window_id].main_pass_id {
            self.repaint_pass_and_child_passes(pass);
        }
    }

    /// Paint `child` before `parent`, which samples its output, keeping what
    /// attached it (see [`Self::attach_child_pass`]): a chain of passes
    /// under one attached pass.
    pub fn set_pass_parent(&mut self, child: DrawPassId, parent: DrawPassId) {
        self.passes[child].parent = CxDrawPassParent::DrawPass(parent);
    }

    /// Parent `child` under `parent` for painting order on behalf of
    /// `attached_by`: the draw list being recorded, whose draw calls consume
    /// the child's output. That list is remembered with its current redraw
    /// id; when it is recorded again without calling this, the child is
    /// orphaned and no longer painted. `None` (no list open) parents without
    /// a record, like `DrawPass::set_pass_parent`.
    pub fn attach_child_pass(
        &mut self,
        child: DrawPassId,
        parent: DrawPassId,
        attached_by: Option<DrawListId>,
    ) {
        let attached_by = attached_by.map(|list_id| (list_id, self.draw_lists[list_id].redraw_id));
        let cxpass = &mut self.passes[child];
        cxpass.parent = CxDrawPassParent::DrawPass(parent);
        cxpass.attached_by = attached_by;
    }

    /// True when the draw list that attached `draw_pass_id` has been recorded
    /// again since without re-attaching it, or was freed: nothing samples the
    /// pass any more. Its own draw list is frozen at the frame that last began
    /// it, and the geometries and textures those draw calls name may since
    /// have been freed and their slots reused — painting it would draw
    /// whatever now sits in them (the gauss scene pass after the window stopped
    /// capturing was re-encoded every pan frame with the map's evicted tile
    /// geometries under other tiles' meshes: tens of millions of triangles into
    /// a texture nobody read).
    pub fn pass_attachment_is_stale(&self, draw_pass_id: DrawPassId) -> bool {
        let Some((list_id, redraw_id)) = self.passes[draw_pass_id].attached_by else {
            return false;
        };
        // A dropped list (its widget is gone) keeps its slot and generation
        // until reuse; that orphans the pass just the same.
        self.draw_lists.is_id_freed(list_id) || self.draw_lists[list_id].redraw_id != redraw_id
    }

    pub fn repaint_pass_and_child_passes(&mut self, draw_pass_id: DrawPassId) {
        let cxpass = &mut self.passes[draw_pass_id];
        cxpass.paint_dirty = true;
        for sub_pass_id in self.passes.id_iter() {
            if let CxDrawPassParent::DrawPass(dep_pass_id) = self.passes[sub_pass_id].parent.clone()
            {
                if dep_pass_id == draw_pass_id {
                    self.repaint_pass_and_child_passes(sub_pass_id);
                }
            }
        }
    }

    pub fn redraw_pass_and_child_passes(&mut self, draw_pass_id: DrawPassId) {
        let cxpass = &self.passes[draw_pass_id];
        if let Some(main_list_id) = cxpass.main_draw_list_id {
            self.redraw_list_and_children(main_list_id);
        }
        // lets redraw all subpasses as well
        for sub_pass_id in self.passes.id_iter() {
            if let CxDrawPassParent::DrawPass(dep_pass_id) = self.passes[sub_pass_id].parent.clone()
            {
                if dep_pass_id == draw_pass_id {
                    self.redraw_pass_and_child_passes(sub_pass_id);
                }
            }
        }
    }

    #[track_caller]
    pub fn redraw_all(&mut self) {
        self.frame_requests += 1;
        crate::frame_sources::note(crate::frame_sources::Kind::Redraw, std::panic::Location::caller());
        self.new_draw_event.redraw_all = true;
    }

    #[track_caller]
    pub fn redraw_area(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            self.redraw_list(draw_list_id);
        }
    }

    pub fn redraw_area_in_draw(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            self.redraw_list_in_draw(draw_list_id);
        }
    }

    #[track_caller]
    pub fn redraw_area_and_children(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            self.redraw_list_and_children(draw_list_id);
        }
    }

    #[track_caller]
    pub fn redraw_list(&mut self, draw_list_id: DrawListId) {
        if self.in_draw_event {
            return;
        }
        self.frame_requests += 1;
        crate::frame_sources::note(crate::frame_sources::Kind::Redraw, std::panic::Location::caller());
        self.redraw_list_in_draw(draw_list_id);
    }

    pub fn redraw_list_in_draw(&mut self, draw_list_id: DrawListId) {
        if self
            .new_draw_event
            .draw_lists
            .iter()
            .position(|v| *v == draw_list_id)
            .is_some()
        {
            return;
        }
        self.new_draw_event.draw_lists.push(draw_list_id);
    }

    #[track_caller]
    pub fn redraw_list_and_children(&mut self, draw_list_id: DrawListId) {
        if self.in_draw_event {
            return;
        }
        self.frame_requests += 1;
        crate::frame_sources::note(crate::frame_sources::Kind::Redraw, std::panic::Location::caller());
        if self
            .new_draw_event
            .draw_lists_and_children
            .iter()
            .position(|v| *v == draw_list_id)
            .is_some()
        {
            return;
        }
        self.new_draw_event
            .draw_lists_and_children
            .push(draw_list_id);
    }

    pub fn get_ime_area_rect(&self) -> Rect {
        self.ime_area.rect(self)
    }

    pub fn update_area_refs(&mut self, old_area: Area, new_area: Area) -> Area {
        if old_area == Area::Empty {
            return new_area;
        }
        if self.ime_area == old_area {
            self.ime_area = new_area;
        }
        self.fingers.update_area(old_area, new_area);
        self.drag_drop.update_area(old_area, new_area);
        self.keyboard.update_area(old_area, new_area);

        new_area
    }

    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.keyboard.set_key_focus(focus_area);
    }

    /// The window lost the keyboard: its key focus is kept (and followed through redraws) for
    /// `unpark_key_focus`, and the live focus is cleared.
    pub fn park_key_focus(&mut self, window: WindowId) {
        self.keyboard.park_key_focus(window);
    }

    pub fn unpark_key_focus(&mut self, window: WindowId) {
        self.keyboard.unpark_key_focus(window);
    }

    pub fn key_focus(&self) -> Area {
        self.keyboard.key_focus()
    }

    pub fn revert_key_focus(&mut self) {
        self.keyboard.revert_key_focus();
    }

    pub fn has_key_focus(&self, focus_area: Area) -> bool {
        self.keyboard.has_key_focus(focus_area)
    }

    #[track_caller]
    pub fn new_next_frame(&mut self) -> NextFrame {
        self.frame_requests += 1;
        crate::frame_sources::note(crate::frame_sources::Kind::NextFrame, std::panic::Location::caller());
        let res = NextFrame(self.next_frame_id);
        self.next_frame_id += 1;
        self.new_next_frames.insert(res);
        res
    }

    /// How many frames were asked for so far: next frames, and redraws
    /// outside a draw. Two readings around some work tell whether it asked
    /// for one (a poll that found nothing changed asks for none).
    pub fn frame_requests(&self) -> u64 {
        self.frame_requests
    }

    /// The frame `next_frame` asked for has not been sent yet. Once it has,
    /// this is false whether or not the asker heard it: a container that
    /// stops passing events on (a closed modal) swallows it, and a widget
    /// running an animation on a frame chain can tell that way that its
    /// chain was cut and re-arm instead of stalling.
    pub fn next_frame_is_pending(&self, next_frame: NextFrame) -> bool {
        self.new_next_frames.contains(&next_frame)
    }

    pub fn send_trigger(&mut self, area: Area, trigger: Trigger) {
        if let Some(triggers) = self.triggers.get_mut(&area) {
            triggers.push(trigger);
        } else {
            let mut new_set = Vec::new();
            new_set.push(trigger);
            self.triggers.insert(area, new_set);
        }
    }

    pub fn set_global<T: 'static + Any + Sized>(&mut self, value: T) {
        if !self.globals.iter().any(|v| v.0 == TypeId::of::<T>()) {
            self.globals.push((TypeId::of::<T>(), Box::new(value)));
        }
    }

    pub fn get_global<T: 'static + Any>(&mut self) -> &mut T {
        let item = self
            .globals
            .iter_mut()
            .find(|v| v.0 == TypeId::of::<T>())
            .unwrap();
        item.1.downcast_mut().unwrap()
    }

    /// Returns an immutable reference to a previously installed Cx-global.
    ///
    /// Unlike [`Cx::get_global`], this accessor does not require mutable access and
    /// does not panic when the requested global has not been installed.
    pub fn get_global_ref<T: 'static + Any>(&self) -> Option<&T> {
        self.globals
            .iter()
            .find(|item| item.0 == TypeId::of::<T>())
            .and_then(|item| item.1.downcast_ref())
    }

    pub fn has_global<T: 'static + Any>(&mut self) -> bool {
        self.globals
            .iter_mut()
            .find(|v| v.0 == TypeId::of::<T>())
            .is_some()
    }

    pub fn global<T: 'static + Any + Default>(&mut self) -> &mut T {
        if !self.has_global::<T>() {
            self.set_global(T::default());
        }
        self.get_global::<T>()
    }

    pub fn spawner(&self) -> &Spawner {
        &self.spawner
    }

    pub fn http_request(&mut self, request_id: LiveId, request: HttpRequest) {
        if self.script_data.std.host_io_only() { return; }
        if let Err(err) = self.net.http_start(request_id, request) {
            crate::error!("http_request failed for {}: {}", request_id.0, err);
        }
    }

    pub fn cancel_http_request(&mut self, request_id: LiveId) {
        if let Err(err) = self.net.http_cancel(request_id) {
            crate::error!("cancel_http_request failed for {}: {}", request_id.0, err);
        }
    }
    /*
        pub fn web_socket_open(&mut self, request_id: LiveId, request: HttpRequest) {
            self.platform_ops.push_back(CxOsOp::WebSocketOpen{
                request,
                request_id,
            });
        }

        pub fn web_socket_send_binary(&mut self, request_id: LiveId, data: Vec<u8>) {
            self.platform_ops.push_back(CxOsOp::WebSocketSendBinary{
                request_id,
                data,
            });
        }
    */
    pub fn prepare_video_playback(
        &mut self,
        video_id: LiveId,
        source: VideoSource,
        camera_preview_mode: CameraPreviewMode,
        external_texture_id: u32,
        texture_id: TextureId,
        autoplay: bool,
        should_loop: bool,
    ) {
        self.prepare_video_playback_with_permission(
            video_id,
            source,
            camera_preview_mode,
            external_texture_id,
            texture_id,
            autoplay,
            should_loop,
            crate::permission::Permission::Camera,
        );
    }

    pub fn prepare_headset_camera_playback(
        &mut self,
        video_id: LiveId,
        source: VideoSource,
        camera_preview_mode: CameraPreviewMode,
        external_texture_id: u32,
        texture_id: TextureId,
        autoplay: bool,
        should_loop: bool,
    ) {
        self.prepare_video_playback_with_permission(
            video_id,
            source,
            camera_preview_mode,
            external_texture_id,
            texture_id,
            autoplay,
            should_loop,
            crate::permission::Permission::HeadsetCamera,
        );
    }

    fn prepare_video_playback_with_permission(
        &mut self,
        video_id: LiveId,
        source: VideoSource,
        camera_preview_mode: CameraPreviewMode,
        external_texture_id: u32,
        texture_id: TextureId,
        autoplay: bool,
        should_loop: bool,
        permission: crate::permission::Permission,
    ) {
        if self.script_data.std.host_io_only() { return; }
        if let VideoSource::Camera(..) = &source {
            self.pending_camera_playbacks
                .push(crate::cx::PendingCameraPlayback {
                    permission,
                    video_id,
                    source,
                    camera_preview_mode,
                    external_texture_id,
                    texture_id,
                    autoplay,
                    should_loop,
                });
            let _request_id = self.request_permission(permission);
            return;
        }
        self.platform_ops.push_back(CxOsOp::PrepareVideoPlayback(
            video_id,
            source,
            camera_preview_mode,
            external_texture_id,
            texture_id,
            autoplay,
            should_loop,
        ));
    }

    pub fn handle_camera_permission_result(
        &mut self,
        result: &crate::permission::PermissionResult,
    ) {
        if !matches!(
            result.permission,
            crate::permission::Permission::Camera | crate::permission::Permission::HeadsetCamera
        ) {
            return;
        }
        let pending: Vec<_> = self.pending_camera_playbacks.drain(..).collect();
        for p in pending {
            if p.permission != result.permission {
                self.pending_camera_playbacks.push(p);
                continue;
            }
            match result.status {
                crate::permission::PermissionStatus::Granted => {
                    self.platform_ops.push_back(CxOsOp::PrepareVideoPlayback(
                        p.video_id,
                        p.source,
                        p.camera_preview_mode,
                        p.external_texture_id,
                        p.texture_id,
                        p.autoplay,
                        p.should_loop,
                    ));
                }
                _ => {
                    self.call_event_handler(&crate::event::Event::VideoDecodingError(
                        crate::event::VideoDecodingErrorEvent {
                            video_id: p.video_id,
                            error: match p.permission {
                                crate::permission::Permission::HeadsetCamera => {
                                    "Headset camera permission denied".to_string()
                                }
                                _ => "Camera permission denied".to_string(),
                            },
                        },
                    ));
                }
            }
        }
    }

    pub fn attach_camera_native_preview(&mut self, video_id: LiveId, area: Area) {
        self.platform_ops
            .push_back(CxOsOp::AttachCameraNativePreview { video_id, area });
    }

    pub fn update_camera_native_preview(&mut self, video_id: LiveId, area: Area, visible: bool) {
        self.platform_ops
            .push_back(CxOsOp::UpdateCameraNativePreview {
                video_id,
                area,
                visible,
            });
    }

    pub fn detach_camera_native_preview(&mut self, video_id: LiveId) {
        self.platform_ops
            .push_back(CxOsOp::DetachCameraNativePreview { video_id });
    }

    pub fn begin_video_playback(&mut self, video_id: LiveId) {
        self.drop_pending_video_transport(video_id);
        self.platform_ops
            .push_back(CxOsOp::BeginVideoPlayback(video_id));
    }

    pub fn pause_video_playback(&mut self, video_id: LiveId) {
        // Last-wins coalescing: one frame should apply a single play/pause
        // intent even though the queue is FIFO.
        self.drop_pending_video_transport(video_id);
        self.platform_ops
            .push_back(CxOsOp::PauseVideoPlayback(video_id));
    }

    pub fn resume_video_playback(&mut self, video_id: LiveId) {
        self.drop_pending_video_transport(video_id);
        self.platform_ops
            .push_back(CxOsOp::ResumeVideoPlayback(video_id));
    }

    pub fn mute_video_playback(&mut self, video_id: LiveId) {
        self.drop_pending_video_mute(video_id);
        self.platform_ops
            .push_back(CxOsOp::MuteVideoPlayback(video_id));
    }

    pub fn unmute_video_playback(&mut self, video_id: LiveId) {
        self.drop_pending_video_mute(video_id);
        self.platform_ops
            .push_back(CxOsOp::UnmuteVideoPlayback(video_id));
    }

    /// Keep only the latest play/pause/begin intent for `video_id`.
    fn drop_pending_video_transport(&mut self, video_id: LiveId) {
        self.platform_ops.retain(|op| match op {
            CxOsOp::BeginVideoPlayback(id)
            | CxOsOp::PauseVideoPlayback(id)
            | CxOsOp::ResumeVideoPlayback(id) => *id != video_id,
            _ => true,
        });
    }

    fn drop_pending_video_mute(&mut self, video_id: LiveId) {
        self.platform_ops.retain(|op| match op {
            CxOsOp::MuteVideoPlayback(id) | CxOsOp::UnmuteVideoPlayback(id) => *id != video_id,
            _ => true,
        });
    }

    pub fn cleanup_video_playback_resources(&mut self, video_id: LiveId) {
        self.platform_ops
            .push_back(CxOsOp::CleanupVideoPlaybackResources(video_id));
    }

    pub fn cancel_pending_camera_playback(&mut self, video_id: LiveId) {
        self.pending_camera_playbacks
            .retain(|pending| pending.video_id != video_id);
    }

    pub fn seek_video_playback(&mut self, video_id: LiveId, position_ms: u64) {
        self.platform_ops
            .push_back(CxOsOp::SeekVideoPlayback(video_id, position_ms));
    }

    pub fn set_video_volume(&mut self, video_id: LiveId, volume: f64) {
        self.platform_ops
            .push_back(CxOsOp::SetVideoVolume(video_id, volume));
    }

    pub fn set_video_playback_rate(&mut self, video_id: LiveId, rate: f64) {
        self.platform_ops
            .push_back(CxOsOp::SetVideoPlaybackRate(video_id, rate));
    }

    pub fn select_video_track(&mut self, video_id: LiveId, index: usize) {
        self.platform_ops
            .push_back(CxOsOp::SelectVideoTrack(video_id, index));
    }

    pub fn select_audio_track(&mut self, video_id: LiveId, index: usize) {
        self.platform_ops
            .push_back(CxOsOp::SelectAudioTrack(video_id, index));
    }

    pub fn prepare_audio_playback(
        &mut self,
        video_id: LiveId,
        source: VideoSource,
        autoplay: bool,
        should_loop: bool,
    ) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops.push_back(CxOsOp::PrepareAudioPlayback(
            video_id,
            source,
            autoplay,
            should_loop,
        ));
    }

    pub fn println_resources(&self) {
        println!("Num textures: {}", self.textures.0.pool.len());
    }

    pub fn open_system_savefile_dialog(&mut self) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops
            .push_back(CxOsOp::SaveFileDialog(FileDialog::new()));
    }

    pub fn open_system_openfile_dialog(&mut self) {
        self.open_select_file_dialog(FileDialog::new());
    }

    /// Open the platform file picker, configured (title, start
    /// location, type filters, multi-select, id) by `dialog`. The answer
    /// arrives later as a [`crate::file_dialogs::FileDialogAction`] in the
    /// actions pass. Native dialogs return `FileSelected` paths by default;
    /// `want_bytes(true)` and every web dialog return `FileLoaded` instead.
    /// On web this should be called directly from a user input handler:
    /// browsers may reject a picker requested after that activation expires.
    pub fn open_select_file_dialog(&mut self, dialog: FileDialog) {
        if self.script_data.std.host_io_only() { return; }
        self.file_dialogs.begin(&dialog);
        self.platform_ops
            .push_back(CxOsOp::SelectFileDialog(dialog));
    }

    /// Set the maximum bytes accepted for one virtual file and for one
    /// multi-file selection/drop. Both limits default to 512 MiB.
    pub fn set_virtual_file_limits(&mut self, max_file_size: u64, max_total_size: u64) {
        let limits = VirtualFileLimits {
            max_file_size,
            max_total_size,
        };
        self.file_dialogs.set_limits(limits);
        #[cfg(target_arch = "wasm32")]
        self.os
            .from_wasm(crate::os::web::from_wasm::FromWasmSetVirtualFileLimits {
                max_file_size: max_file_size as f64,
                max_total_size: max_total_size as f64,
            });
    }

    pub fn virtual_file_limits(&self) -> VirtualFileLimits {
        self.file_dialogs.limits()
    }

    /// Open the platform's native save panel. The OS asks about
    /// overwriting before answering `SaveFileSelected`.
    pub fn open_save_file_dialog(&mut self, dialog: FileDialog) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops.push_back(CxOsOp::SaveFileDialog(dialog));
    }

    pub fn open_system_savefolder_dialog(&mut self) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops
            .push_back(CxOsOp::SaveFolderDialog(FileDialog::new()));
    }

    pub fn open_system_openfolder_dialog(&mut self) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops
            .push_back(CxOsOp::SelectFolderDialog(FileDialog::new()));
    }

    /// Open the platform's native folder picker, configured (title, start
    /// location) by `dialog`. The answer arrives later as a
    /// [`crate::file_dialogs::FileDialogAction`] in the actions pass —
    /// `FolderSelected` with the chosen path, or `FolderCancelled`.
    pub fn open_select_folder_dialog(&mut self, dialog: FileDialog) {
        if self.script_data.std.host_io_only() { return; }
        self.platform_ops
            .push_back(CxOsOp::SelectFolderDialog(dialog));
    }

    pub fn event_id(&self) -> u64 {
        self.event_id
    }
}

/// Returns the canPlayType string for the given MIME type on the current platform.
/// Possible values: `""` (cannot play), `"maybe"`, `"probably"`.
pub fn can_play_type(mime: &str) -> &'static str {
    can_play_type_impl(mime)
}

#[cfg(test)]
mod stale_window_tests {
    use super::*;
    use crate::window::WindowHandle;

    #[test]
    fn immutable_global_accessor_is_optional_and_preserves_the_value() {
        let mut cx = Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}));
        assert_eq!(cx.get_global_ref::<u64>(), None);
        cx.set_global(41_u64);
        assert_eq!(cx.get_global_ref::<u64>(), Some(&41));
    }

    /// A window the HAL reports at 930×848 points and dpi 2 whose app
    /// shrank its own dpi to 1.6: it lays out 1.25× larger, and a host
    /// pointer at (100, 100) must land at (125, 125) in its points.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_hosted_window_with_a_dpi_override_lays_out_larger_and_remaps_the_host_pointer() {
        let mut cx = Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}));
        let window = WindowHandle::new(&mut cx);
        let window_id = window.window_id();
        cx.windows[window_id].is_created = true;
        let restore = makepad_hal::window_state::RESTORE;
        cx.hal_window_geom(window_id, dvec2(0.0, 0.0), dvec2(930.0, 848.0), 2.0, restore);
        assert_eq!(cx.windows[window_id].window_geom.inner_size, dvec2(930.0, 848.0));
        cx.windows[window_id].dpi_override = Some(1.6);
        cx.hal_window_geom(window_id, dvec2(0.0, 0.0), dvec2(930.0, 848.0), 2.0, restore);
        let again = cx.windows[window_id].window_geom.clone();
        assert!(
            (again.inner_size.x - 1162.5).abs() < 1e-9,
            "{:?}",
            again.inner_size
        );
        assert!((again.inner_size.y - 1060.0).abs() < 1e-9);
        assert_eq!(again.dpi_factor, 1.6);
        let mut pos = dvec2(100.0, 100.0);
        cx.dpi_override_scale(&mut pos, window_id);
        assert!(
            (pos.x - 125.0).abs() < 1e-9 && (pos.y - 125.0).abs() < 1e-9,
            "{pos:?}"
        );
        // The host's point (900, 800) is still inside the window: containment is in host points.
        let (hit, origin) = cx.windows.window_id_contains(dvec2(900.0, 800.0));
        assert_eq!(hit, window_id);
        assert_eq!(origin, dvec2(0.0, 0.0));
    }

    #[test]
    fn dpi_override_ignores_closed_and_out_of_range_windows() {
        let mut cx = Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}));
        let window = WindowHandle::new(&mut cx);
        let window_id = window.window_id();
        cx.windows[window_id].is_created = true;
        cx.windows[window_id].os_dpi_factor = Some(2.0);
        cx.windows[window_id].dpi_override = Some(1.0);

        let mut live_pos = dvec2(12.0, 8.0);
        cx.dpi_override_scale(&mut live_pos, window_id);
        assert_eq!(live_pos, dvec2(24.0, 16.0));

        cx.windows[window_id].is_created = false;
        let mut closed_pos = dvec2(12.0, 8.0);
        cx.dpi_override_scale(&mut closed_pos, window_id);
        assert_eq!(closed_pos, dvec2(12.0, 8.0));

        let mut stale_pos = dvec2(12.0, 8.0);
        cx.dpi_override_scale(&mut stale_pos, WindowId(usize::MAX, u64::MAX));
        assert_eq!(stale_pos, dvec2(12.0, 8.0));
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "windows", target_os = "android"))]
fn can_play_type_impl(mime: &str) -> &'static str {
    makepad_hal::can_play_type(mime)
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "windows", target_os = "android")))]
fn can_play_type_impl(_mime: &str) -> &'static str { "" }

#[macro_export]
macro_rules! register_component_factory {
    ( $ cx: expr, $ registry: ident, $ ty: ty, $ factory: ident) => {
        let module_id = LiveModuleId::from_str(&module_path!()).unwrap();
        if let Some((reg, _)) = $cx
            .live_registry
            .borrow()
            .components
            .get_or_create::<$registry>()
            .map
            .get(&LiveType::of::<$ty>())
        {
            if reg.module_id != module_id {
                panic!(
                    "Component already registered {} {}",
                    stringify!($ty),
                    reg.module_id
                );
            }
        }
        $cx.live_registry
            .borrow()
            .components
            .get_or_create::<$registry>()
            .map
            .insert(
                LiveType::of::<$ty>(),
                (
                    LiveComponentInfo {
                        name: LiveId::from_str_with_lut(stringify!($ty)).unwrap(),
                        module_id,
                    },
                    Box::new($factory()),
                ),
            );
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A texture pass and its render target come out the same pixel size for every scale and
    /// window size: the target sized with `pass_pixel_size` of the logical size the pass is set
    /// to, the pass by `get_pass_rect` (snapped to `pass_logical_size`) and then the frame's
    /// pass size (`pass_pixel_size` again). A target floored where the pass was ceiled was a
    /// pixel short at 1.25 / 1.41 / 1.75 and the GPU refused the pass (149, Scope's Glass).
    #[test]
    fn a_pass_is_exactly_its_target_at_every_scale_and_size() {
        let sizes = [1.0, 7.0, 99.5, 333.0, 640.0, 875.25, 876.0, 1179.0, 1180.0, 1235.7, 1920.0, 2559.0, 4097.3];
        let mut checked = 0;
        for step in 100..=250 {
            let dpi = step as f64 / 100.0;
            for &w in &sizes {
                for &h in &sizes {
                    for logical in [dvec2(w, h), dvec2(w, h) * (1.0 - 1e-12)] {
                        let target = Cx::pass_pixel_size(logical, dpi);
                        // the pass: snapped in points, then the frame's pass size from that
                        let pass = Cx::pass_pixel_size(Cx::pass_logical_size(logical, dpi), dpi);
                        assert_eq!(pass, target, "logical {logical:?} at dpi {dpi}");
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 151 * 13 * 13 * 2);
    }

    #[test]
    fn nested_overlay_locks_unwind_one_level_at_a_time() {
        let mut cx = Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}));
        let list = cx.draw_lists.alloc();
        let area = |rect_id| {
            Area::Rect(crate::area::RectArea {
                draw_list_id: list.id(),
                rect_id,
                redraw_id: 0,
            })
        };
        let (dialog, popover) = (area(0), area(1));

        cx.sweep_lock(dialog);
        cx.block_scrolling_except_within(dialog);
        cx.sweep_lock(popover);
        cx.block_scrolling_except_within(popover);
        assert_eq!(cx.sweep_lock_area(), Some(popover));
        assert_eq!(cx.fingers.blocked_scrolling_exception_area(), Some(popover));

        cx.sweep_unlock(popover);
        cx.unblock_scrolling_within_area(popover);
        assert_eq!(cx.sweep_lock_area(), Some(dialog));
        assert_eq!(cx.fingers.blocked_scrolling_exception_area(), Some(dialog));

        cx.sweep_unlock(dialog);
        cx.unblock_scrolling();
        assert_eq!(cx.sweep_lock_area(), None);
        assert_eq!(cx.fingers.blocked_scrolling_exception_area(), None);
    }

}

/// How many asynchronous loads of one kind a subsystem still has in flight:
/// work started on a worker or the network whose result will change what
/// the app draws once it lands (an image decode, a glyph raster, a fetched
/// resource).
pub type AsyncLoadProbe = fn(&Cx) -> usize;

impl Cx {
    /// Let a subsystem report its asynchronous loads in flight under `kind`
    /// ("image", "glyph", ...). Registering a kind again replaces its probe.
    /// A probe must only count, cheaply: it runs when someone asks
    /// (`pending_async_loads`), never per frame.
    pub fn register_async_load_probe(&mut self, kind: &'static str, probe: AsyncLoadProbe) {
        match self.async_load_probes.iter_mut().find(|(name, _)| *name == kind) {
            Some(entry) => entry.1 = probe,
            None => self.async_load_probes.push((kind, probe)),
        }
    }

    /// Asynchronous loads still in flight, by kind, only the kinds with
    /// any: what a frame drawn now would not show yet. Script resources
    /// (files and fetches Splash asked for) are counted here as
    /// "resource"; image decodes, glyph rasters and the like by the
    /// subsystems that registered a probe. Texture uploads are not loads:
    /// a backend uploads a texture in the frame that first samples it.
    /// Frames presented with a glyph drawn blank: the web build rasterises its glyphs in the
    /// draw (natively the HAL's font service answers after it, see `os::hal::fonts`).
    #[cfg(target_arch = "wasm32")]
    pub fn blank_glyph_frames(&self) -> u64 {
        0
    }

    pub fn pending_async_loads(&self) -> Vec<(&'static str, usize)> {
        let mut pending = Vec::new();
        let resources = self
            .script_data
            .resources
            .resources
            .try_borrow()
            .map(|resources| {
                resources
                    .iter()
                    .filter(|resource| {
                        matches!(resource.data, crate::script::res::CxScriptResourceData::Loading)
                    })
                    .count()
            })
            .unwrap_or(0)
            .max(self.script_data.resources.http_resources.len());
        if resources > 0 {
            pending.push(("resource", resources));
        }
        for (kind, probe) in &self.async_load_probes {
            let count = probe(self);
            if count > 0 {
                pending.push((*kind, count));
            }
        }
        pending
    }
}

impl Cx {
    /// Drain completed tickets without waiting. Call on Event::Signal, also
    /// when no pass needs repainting. Returned Arc payloads belong to the
    /// caller and are no longer charged to the platform's result budget.
    pub fn try_take_texture_readbacks(&mut self) -> Vec<crate::texture::TextureReadback> {
        self.take_texture_readback_results(false)
            .into_iter()
            .map(|(_, result)| result)
            .collect()
    }

    /// Drain completed readbacks of these tickets only, leaving every other
    /// ticket for its own requester. Several consumers in one process (a
    /// thumbnail renderer and a frame exporter, say) each call this with
    /// the tickets their `Texture::read_back` calls returned; a finished
    /// ticket is returned once and then forgotten. The same results and
    /// budget accounting as [`Cx::try_take_texture_readbacks`], which
    /// takes all of them and suits a process with a single consumer.
    pub fn try_take_texture_readbacks_for(
        &mut self,
        tickets: &[crate::texture::ReadbackTicket],
    ) -> Vec<crate::texture::TextureReadback> {
        if tickets.is_empty() {
            return Vec::new();
        }
        self.take_texture_readback_results_where(false, |ticket| tickets.contains(&ticket))
            .into_iter()
            .map(|(_, result)| result)
            .collect()
    }

    pub(crate) fn take_texture_readback_results(
        &mut self,
        legacy: bool,
    ) -> Vec<(TextureId, crate::texture::TextureReadback)> {
        self.take_texture_readback_results_where(legacy, |_| true)
    }

    fn take_texture_readback_results_where(
        &mut self,
        legacy: bool,
        wanted: impl Fn(crate::texture::ReadbackTicket) -> bool,
    ) -> Vec<(TextureId, crate::texture::TextureReadback)> {
        use crate::texture::ReadbackError;
        if self.textures.1.readbacks.slots.is_empty() {
            return Vec::new();
        }
        self.validate_pending_readbacks();
        self.poll_texture_readbacks();
        let state = &mut self.textures.1.readbacks;
        let mut results = Vec::new();
        let mut index = 0;
        while index < state.slots.len() {
            let slot = &mut state.slots[index];
            if slot.legacy != legacy || !wanted(slot.result.ticket) {
                index += 1;
                continue;
            }
            if let Some(receive) = &slot.receive {
                match receive.try_recv() {
                    Ok(result) => {
                        slot.result.data = result;
                        slot.receive = None;
                    }
                    Err(crate::makepad_network::mpsc::TryRecvError::Disconnected) => {
                        slot.result.data = Err(ReadbackError::DeviceLost);
                        slot.receive = None;
                    }
                    Err(crate::makepad_network::mpsc::TryRecvError::Empty) => {}
                }
            }
            if !slot.pending && slot.receive.is_none() {
                let mut slot = state.slots.remove(index);
                state.reserved_bytes -= slot.reserved_bytes;
                if slot.cancelled {
                    slot.result.data = Err(ReadbackError::Cancelled);
                }
                results.push((slot.texture.texture_id(), slot.result));
            } else {
                index += 1;
            }
        }
        results
    }

    /// Cancel publication. An in-flight allocation stays pinned until its
    /// copy/lease finishes; the ticket then yields Cancelled instead of bytes.
    pub fn cancel_texture_readback(&mut self, ticket: crate::texture::ReadbackTicket) -> bool {
        let Some(slot) = self
            .textures
            .1
            .readbacks
            .slots
            .iter_mut()
            .find(|slot| slot.result.ticket == ticket)
        else {
            return false;
        };
        slot.cancelled = true;
        if slot.pending {
            slot.pending = false;
        }
        crate::thread::SignalToUI::set_ui_signal();
        true
    }

    pub fn texture_readback_usage(&self) -> crate::texture::TextureReadbackUsage {
        crate::texture::TextureReadbackUsage {
            requests: self.textures.1.readbacks.slots.len(),
            reserved_bytes: self.textures.1.readbacks.reserved_bytes,
        }
    }

    /// Last submitted renderer serial. Recording a Draw event does not advance
    /// this value. See `Texture` for backend units and ordering guarantees.
    pub fn frame_submission_serial(&self) -> u64 {
        self.textures
            .1
            .serials
            .submitted
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Publish an immutable block of instance data (stride `slots`) as a
    /// shared publication. The block is charged against the device envelope
    /// and refused with `PublishError::NoRoom` when it does not fit; the
    /// receipt starts `Pending` and the backend marks it ready once the
    /// backing copy landed. Draw items reference `(block, first, count)`.
    pub fn publish_instances(
        &self,
        slots: usize,
        data: std::sync::Arc<[f32]>,
        hints: crate::shared_instances::PublishHints,
    ) -> Result<crate::shared_instances::SharedInstances, crate::shared_instances::PublishError> {
        self.publications.publish(slots, data, hints)
    }

    /// The context's publication totals: charged, pending retirement, the
    /// derived envelope and the live count.
    pub fn publication_accounting(&self) -> crate::shared_instances::PublicationAccounting {
        self.publications.accounting()
    }

    /// The producer's pacing input for this frame: room under the envelope
    /// plus the backend's last copy observation (recorded by the backend
    /// through `Publications::record_observation`), against the time the
    /// caller still has in the frame.
    pub fn publish_backpressure(
        &self,
        frame_remaining_ns: u64,
    ) -> crate::shared_instances::PublishBackpressure {
        self.publications.backpressure(frame_remaining_ns)
    }

    /// Set the publication envelope from the machine's numbers
    /// (`retained_instances::retained_device_envelope`), once the backend
    /// knows them. Zero (unknown) refuses nothing.
    pub fn set_publication_envelope(&self, bytes: usize) {
        self.publications.set_envelope(bytes);
    }

    /// Whether a pass submitted with a serial in `first..=last` completed with draws left out:
    /// a pipeline they use was still being built for its target's layout (the HAL's
    /// `FramePartial`; they come with a later paint). A pass's owner sees it painted when it was
    /// built, so an offscreen render that takes its pictures checks this once the frame's passes
    /// completed (`Texture::gpu_drawn`) and draws the frame again. The latest 256 are kept.
    pub fn frames_partial(&self, first: u64, last: u64) -> bool {
        self.textures.1.partial.iter().any(|serial| (first..=last).contains(serial))
    }

    /// Last observed completed prefix, without polling the backend. Progress
    /// timers use `frame_completion_serial` to refresh this nonblocking snapshot.
    pub fn frame_completed_serial(&self) -> u64 {
        self.textures.1.serials.completed.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Poll without waiting, collect finished texture retirements, and return
    /// the greatest serial whose entire prefix has completed. Call again while
    /// pending, even when the application does not need to repaint.
    pub fn frame_completion_serial(&mut self) -> u64 {
        self.poll_texture_lifetimes();
        self.textures
            .1
            .serials
            .completed
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Allocated texture bytes, including free pool slots and pending releases.
    /// Unknown externally owned allocations are omitted. This is an on-demand
    /// O(pool size) measurement; ordinary rendering does not scan the pool.
    #[cfg(target_arch = "wasm32")]
    pub fn texture_pool_bytes(&self) -> u64 {
        self.textures.0.pool.iter().enumerate().fold(
            self.textures.1.retired.iter().fold(0u64, |sum, retired| {
                sum.saturating_add(retired.os.allocated_bytes(self).unwrap_or(retired.bytes))
            }),
            |sum, (index, slot)| {
                sum.saturating_add(
                    self.texture_allocation_bytes(TextureId::from_pool_slot(
                        index,
                        slot.generation,
                    ))
                    .unwrap_or(0),
                )
                .saturating_add(
                    slot.item
                        .previous_platform_resource
                        .as_ref()
                        .and_then(|os| os.allocated_bytes(self))
                        .unwrap_or(0),
                )
            },
        )
    }

}

#[cfg(test)]
mod host_io_tests {
    use super::*;

    #[test]
    fn host_io_native_widget_paths_do_not_enqueue_external_operations() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.script_data.std.restrict_to_host_io();
        let before = cx.platform_ops.len();
        cx.copy_to_clipboard("private");
        cx.set_primary_selection("private");
        cx.open_system_savefile_dialog();
        cx.open_system_openfile_dialog();
        cx.open_system_openfolder_dialog();
        cx.open_system_savefolder_dialog();
        cx.system_browser(LiveId::unique()).spawn("https://example.invalid/private");
        cx.system_browser(LiveId::unique()).set_url("https://example.invalid/private", false);
        cx.prepare_audio_playback(LiveId::unique(), VideoSource::Network("https://example.invalid/private".into()), false, false);
        cx.prepare_audio_playback(LiveId::unique(), VideoSource::Filesystem("/etc/passwd".into()), false, false);
        assert_eq!(cx.platform_ops.len(), before);
    }
}

#[cfg(test)]
mod readback_routing_tests {
    use super::*;
    use crate::draw_pass::{DrawPass, DrawPassClearColor};
    use crate::texture::{ReadbackError, ReadbackRequest, TextureFormat, TextureSize};

    /// A render target with a pass that has yet to paint it, so a
    /// readback is admitted and stays pending until the pass renders.
    fn pending_target(cx: &mut Cx) -> (Texture, DrawPass) {
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed { width: 8, height: 8 },
                initial: true,
            },
        );
        let pass = DrawPass::new(cx);
        pass.add_color_texture(cx, &texture, DrawPassClearColor::ClearWith(crate::makepad_math::vec4(0.0, 0.0, 0.0, 1.0)));
        pass.set_size(cx, dvec2(8.0, 8.0));
        cx.passes[pass.draw_pass_id()].dpi_factor = Some(1.0);
        cx.passes[pass.draw_pass_id()].paint_dirty = true;
        (texture, pass)
    }

    #[test]
    fn readbacks_go_to_the_requester_holding_the_ticket() {
        // Two consumers in one process: each takes only its own tickets,
        // once; the take-all API still drains whatever is left.
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let targets: Vec<_> = (0..3).map(|_| pending_target(&mut cx)).collect();
        let tickets: Vec<_> = targets
            .iter()
            .map(|(texture, _)| texture.read_back(&mut cx, ReadbackRequest::default()).unwrap())
            .collect();
        assert!(cx.try_take_texture_readbacks_for(&tickets).is_empty(), "still pending");
        // Cancellation finishes a ticket without a GPU: its result is final.
        for ticket in &tickets {
            assert!(cx.cancel_texture_readback(*ticket));
        }
        let first = cx.try_take_texture_readbacks_for(&tickets[..1]);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].ticket, tickets[0]);
        assert!(matches!(first[0].data, Err(ReadbackError::Cancelled)));
        assert!(cx.try_take_texture_readbacks_for(&tickets[..1]).is_empty(), "taken once");
        let second = cx.try_take_texture_readbacks_for(&tickets[1..2]);
        assert_eq!(second.iter().map(|r| r.ticket).collect::<Vec<_>>(), vec![tickets[1]]);
        assert!(cx.try_take_texture_readbacks_for(&[]).is_empty());
        let rest = cx.try_take_texture_readbacks();
        assert_eq!(rest.iter().map(|r| r.ticket).collect::<Vec<_>>(), vec![tickets[2]]);
        assert_eq!(cx.texture_readback_usage().reserved_bytes, 0);
    }
}

#[cfg(test)]
mod async_load_tests {
    use super::*;

    #[test]
    fn pending_async_loads_sum_registered_probes_by_kind() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert!(cx.pending_async_loads().is_empty());
        cx.register_async_load_probe("image", |_| 2);
        cx.register_async_load_probe("glyph", |_| 0);
        assert_eq!(cx.pending_async_loads(), vec![("image", 2)]);
        // registering a kind again replaces its probe
        cx.register_async_load_probe("image", |_| 0);
        cx.register_async_load_probe("glyph", |_| 5);
        assert_eq!(cx.pending_async_loads(), vec![("glyph", 5)]);
    }

    #[test]
    fn frame_requests_count_next_frames_and_redraws_outside_a_draw() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let start = cx.frame_requests();
        cx.new_next_frame();
        cx.redraw_all();
        assert_eq!(cx.frame_requests(), start + 2);
        // a redraw asked inside a draw is spent by that draw: no request
        let list = crate::draw_list::DrawList::new(&mut cx);
        cx.in_draw_event = true;
        cx.redraw_list(list.id());
        cx.in_draw_event = false;
        assert_eq!(cx.frame_requests(), start + 2);
    }
}
