use crate::file_dialogs::FileDialog;

use {
    crate::{
        area::Area,
        cursor::MouseCursor,
        cx::{Cx, CxRef, OsType, XrCapabilities},
        draw_list::DrawListId,
        draw_pass::{CxDrawPassParent, CxDrawPassRect, DrawPassId},
        dvec2,
        event::keyboard::CharOffset,
        event::xr::XrAnchor,
        event::{
            video_playback::CameraPreviewMode, DragItem, NextFrame, Timer, Trigger, VideoSource,
        },
        gpu_info::GpuInfo,
        ime::TextInputConfig,
        macos_menu::MacosMenu,
        makepad_futures::executor::Spawner,
        makepad_live_id::*,
        makepad_math::{Rect, Vec2d},
        makepad_network::HttpRequest,
        makepad_script::value::ScriptHandle,
        texture::{Texture, TextureId},
        window::WindowId,
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
pub trait CxOsApi {
    fn init_cx_os(&mut self);

    fn spawn_thread<F>(&mut self, f: F)
    where
        F: FnOnce() + Send + 'static;

    fn start_stdin_service(&mut self) {}
    fn pre_start() -> bool {
        false
    }

    fn open_url(&mut self, url: &str, in_place: OpenUrlInPlace);

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

#[derive(PartialEq)]
pub enum CxOsOp {
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
    SetTopmost(WindowId, bool),
    ShowInDock(bool),

    ShowTextIME(Area, Vec2d, TextInputConfig),
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
    UpdateMacosMenu(MacosMenu),
    ShowClipboardActions {
        has_selection: bool,
        rect: Rect,
        keyboard_shift: f64,
    },
    HideClipboardActions,
    CopyToClipboard(String),
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

    HttpRequest {
        request_id: LiveId,
        request: HttpRequest,
    },
    CancelHttpRequest {
        request_id: LiveId,
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
    UpdateVideoSurfaceTexture(LiveId),

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
    XrSetLocalAnchor(XrAnchor),
    XrAdvertiseAnchor(XrAnchor),
    XrDiscoverAnchor(u8),
    XrStopPresenting,
}

impl std::fmt::Debug for CxOsOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateWindow(..) => write!(f, "CreateWindow"),
            Self::CreatePopupWindow { .. } => write!(f, "CreatePopupWindow"),
            Self::CloseWindow(..) => write!(f, "CloseWindow"),
            Self::MinimizeWindow(..) => write!(f, "MinimizeWindow"),
            Self::Deminiaturize(..) => write!(f, "Deminiaturize"),
            Self::MaximizeWindow(..) => write!(f, "MaximizeWindow"),
            Self::FullscreenWindow(..) => write!(f, "FullscreenWindow"),
            Self::NormalizeWindow(..) => write!(f, "NormalizeWindow"),
            Self::RestoreWindow(..) => write!(f, "RestoreWindow"),
            Self::HideWindow(..) => write!(f, "HideWindow"),
            Self::HideWindowButtons(..) => write!(f, "HideWindowButtons"),
            Self::ShowWindowButtons(..) => write!(f, "ShowWindowButtons"),
            Self::SetTopmost(..) => write!(f, "SetTopmost"),
            Self::ShowInDock(..) => write!(f, "ShowInDock"),

            Self::ShowTextIME(..) => write!(f, "ShowTextIME"),
            Self::HideTextIME => write!(f, "HideTextIME"),
            Self::SyncImeState { .. } => write!(f, "SyncImeState"),
            Self::SetCursor(..) => write!(f, "SetCursor"),
            Self::StartTimer { .. } => write!(f, "StartTimer"),
            Self::StopTimer(..) => write!(f, "StopTimer"),
            Self::Quit => write!(f, "Quit"),

            Self::StartDragging(..) => write!(f, "StartDragging"),
            Self::UpdateMacosMenu(..) => write!(f, "UpdateMacosMenu"),
            Self::ShowClipboardActions { .. } => write!(f, "ShowClipboardActions"),
            Self::HideClipboardActions => write!(f, "HideClipboardActions"),
            Self::CopyToClipboard(..) => write!(f, "CopyToClipboard"),
            Self::SetPrimarySelection(..) => write!(f, "SetPrimarySelection"),
            Self::ShowSelectionHandles { .. } => write!(f, "ShowSelectionHandles"),
            Self::UpdateSelectionHandles { .. } => write!(f, "UpdateSelectionHandles"),
            Self::HideSelectionHandles => write!(f, "HideSelectionHandles"),
            Self::AccessibilityUpdate(..) => write!(f, "AccessibilityUpdate"),

            Self::CheckPermission { .. } => write!(f, "CheckPermission"),
            Self::RequestPermission { .. } => write!(f, "RequestPermission"),

            Self::HttpRequest { .. } => write!(f, "HttpRequest"),
            Self::CancelHttpRequest { .. } => write!(f, "CancelHttpRequest"),

            Self::PrepareVideoPlayback(..) => write!(f, "PrepareVideoPlayback"),
            Self::AttachCameraNativePreview { .. } => write!(f, "AttachCameraNativePreview"),
            Self::UpdateCameraNativePreview { .. } => write!(f, "UpdateCameraNativePreview"),
            Self::DetachCameraNativePreview { .. } => write!(f, "DetachCameraNativePreview"),
            Self::PrepareAudioPlayback(..) => write!(f, "PrepareAudioPlayback"),
            Self::BeginVideoPlayback(..) => write!(f, "BeginVideoPlayback"),
            Self::PauseVideoPlayback(..) => write!(f, "PauseVideoPlayback"),
            Self::ResumeVideoPlayback(..) => write!(f, "ResumeVideoPlayback"),
            Self::MuteVideoPlayback(..) => write!(f, "MuteVideoPlayback"),
            Self::UnmuteVideoPlayback(..) => write!(f, "UnmuteVideoPlayback"),
            Self::CleanupVideoPlaybackResources(..) => write!(f, "CleanupVideoPlaybackResources"),
            Self::SeekVideoPlayback(..) => write!(f, "SeekVideoPlayback"),
            Self::SetVideoVolume(..) => write!(f, "SetVideoVolume"),
            Self::SetVideoPlaybackRate(..) => write!(f, "SetVideoPlaybackRate"),
            Self::UpdateVideoSurfaceTexture(..) => write!(f, "UpdateVideoSurfaceTexture"),
            Self::CreateWebView { .. } => write!(f, "CreateWebView"),
            Self::UpdateWebView { .. } => write!(f, "UpdateWebView"),
            Self::CloseWebView { .. } => write!(f, "CloseWebView"),
            Self::SaveFileDialog(..) => write!(f, "SaveFileDialog"),
            Self::SelectFileDialog(..) => write!(f, "SelectFileDialog"),
            Self::SaveFolderDialog(..) => write!(f, "SaveFolderDialog"),
            Self::SelectFolderDialog(..) => write!(f, "SelectFolderDialog"),
            Self::ResizeWindow(..) => write!(f, "ResizeWindow"),
            Self::RepositionWindow(..) => write!(f, "RepositionWindow"),

            Self::XrStartPresenting => write!(f, "XrStartPresenting"),
            Self::XrStopPresenting => write!(f, "XrStopPresenting"),
            Self::XrAdvertiseAnchor(_) => write!(f, "XrAdvertiseAnchor"),
            Self::XrSetLocalAnchor(_) => write!(f, "XrSetLocalAnchor"),
            Self::XrDiscoverAnchor(_) => write!(f, "XrDiscoverAnchor"),
        }
    }
}
impl Cx {
    pub fn in_draw_event(&self) -> bool {
        self.in_draw_event
    }

    pub fn xr_capabilities(&self) -> &XrCapabilities {
        &self.xr_capabilities
    }

    pub fn get_ref(&self) -> CxRef {
        CxRef(self.self_ref.clone().unwrap())
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
        {
            if let Some(data) =
                unsafe { crate::os::linux::android::android_jni::to_java_load_asset(path) }
            {
                return Ok(Rc::new(data));
            }
            if let Some(package_root) = self.package_root.as_deref() {
                let root_prefix = format!("{}/", package_root);
                if !path.starts_with(&root_prefix) {
                    let prefixed_path = format!("{}/{}", package_root, path);
                    if let Some(data) = unsafe {
                        crate::os::linux::android::android_jni::to_java_load_asset(&prefixed_path)
                    } {
                        return Ok(Rc::new(data));
                    }
                }
            }
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
        {
            if let Some(data) =
                unsafe { crate::os::linux::android::android_jni::to_java_load_asset(path) }
            {
                return Ok(Rc::new(data));
            }
            if let Some(package_root) = self.package_root.as_deref() {
                let root_prefix = format!("{}/", package_root);
                if !path.starts_with(&root_prefix) {
                    let prefixed_path = format!("{}/{}", package_root, path);
                    if let Some(data) = unsafe {
                        crate::os::linux::android::android_jni::to_java_load_asset(&prefixed_path)
                    } {
                        return Ok(Rc::new(data));
                    }
                }
            }
        }

        Err(format!("Dependency not loaded {}", path))
    }

    /// Get loaded resource data by ScriptHandle
    pub fn get_resource(&self, handle: ScriptHandle) -> Option<Rc<Vec<u8>>> {
        if let Some(data) = self.script_data.resources.get_data(handle) {
            return Some(data);
        }

        // On web, resources are also available through the dependency table that
        // arrives during ToWasmInit. If a script resource hasn't been promoted to
        // Loaded yet, allow direct dependency lookup as a synchronous fallback.
        if self.os_type().is_web() {
            let resources = self.script_data.resources.resources.borrow();
            if let Some(res) = resources.iter().find(|res| res.handle == handle) {
                if let Some(dep_path) = res.dependency_path.as_deref() {
                    if let Ok(data) = self.get_dependency(dep_path) {
                        return Some(data);
                    }
                }
            }
        }

        None
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

    pub fn os_type(&self) -> &OsType {
        &self.os_type
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
        self.platform_ops.push(CxOsOp::UpdateMacosMenu(menu));
    }

    pub fn xr_start_presenting(&mut self) {
        self.platform_ops.push(CxOsOp::XrStartPresenting);
    }

    pub fn xr_advertise_anchor(&mut self, anchor: XrAnchor) {
        self.platform_ops.push(CxOsOp::XrAdvertiseAnchor(anchor));
    }

    pub fn xr_set_local_anchor(&mut self, anchor: XrAnchor) {
        self.platform_ops.push(CxOsOp::XrSetLocalAnchor(anchor));
    }

    pub fn xr_discover_anchor(&mut self, id: u8) {
        self.platform_ops.push(CxOsOp::XrDiscoverAnchor(id));
    }

    pub fn quit(&mut self) {
        self.platform_ops.push(CxOsOp::Quit);
    }
    // Determines whether to show your application in the dock when it runs. The default value is true.
    // You can remove the dock icon by setting this value to false.
    pub fn show_in_dock(&mut self, show: bool) {
        self.platform_ops.push(CxOsOp::ShowInDock(show));
    }
    pub fn push_unique_platform_op(&mut self, op: CxOsOp) {
        if self.platform_ops.iter().find(|o| **o == op).is_none() {
            self.platform_ops.push(op);
        }
    }

    pub fn show_text_ime(&mut self, area: Area, pos: Vec2d) {
        self.show_text_ime_with_config(area, pos, TextInputConfig::default());
    }

    pub fn show_text_ime_with_config(&mut self, area: Area, pos: Vec2d, config: TextInputConfig) {
        if !self.keyboard.text_ime_dismissed {
            self.ime_area = area;
            self.platform_ops
                .push(CxOsOp::ShowTextIME(area, pos, config));
        }
    }

    pub fn sync_ime_state(
        &mut self,
        text: String,
        selection: Range<CharOffset>,
        composition: Option<Range<CharOffset>>,
    ) {
        self.platform_ops.push(CxOsOp::SyncImeState {
            text,
            selection,
            composition,
        });
    }

    pub fn hide_text_ime(&mut self) {
        self.keyboard.reset_text_ime_dismissed();
        self.platform_ops.push(CxOsOp::HideTextIME);
    }

    pub fn text_ime_was_dismissed(&mut self) {
        self.keyboard.set_text_ime_dismissed();
        self.platform_ops.push(CxOsOp::HideTextIME);
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
    /// * `rect` - Selection bounding box in logical pixels (for menu positioning)
    /// * `keyboard_shift` - Vertical offset caused by virtual keyboard (in logical pixels)
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
        self.platform_ops.push(CxOsOp::ShowClipboardActions {
            has_selection,
            rect,
            keyboard_shift,
        });
    }

    /// Hides the clipboard actions menu
    pub fn hide_clipboard_actions(&mut self) {
        self.platform_ops.push(CxOsOp::HideClipboardActions);
    }

    /// Copies the given string to the clipboard.
    ///
    /// Due to lack of platform clipboard support, it does not work on Web or tvOS.
    pub fn copy_to_clipboard(&mut self, content: &str) {
        self.platform_ops
            .push(CxOsOp::CopyToClipboard(content.to_owned()));
    }

    /// Sets the primary selection (Linux middle-click paste).
    /// No-op on non-Linux platforms.
    pub fn set_primary_selection(&mut self, content: &str) {
        self.platform_ops
            .push(CxOsOp::SetPrimarySelection(content.to_owned()));
    }

    /// Forward an accessibility tree update to the platform adapter.
    ///
    /// The `update` is a type-erased `accesskit::TreeUpdate`. Platform backends
    /// downcast it when an accessibility adapter is active.
    pub fn update_accessibility_tree(&mut self, update: Box<dyn std::any::Any + Send>) {
        self.platform_ops
            .push(CxOsOp::AccessibilityUpdate(AccessibilityUpdatePayload(
                update,
            )));
    }

    /// Show native selection handles at the given start and end positions (mobile).
    pub fn show_selection_handles(&mut self, start: Vec2d, end: Vec2d) {
        self.platform_ops
            .push(CxOsOp::ShowSelectionHandles { start, end });
    }

    /// Update positions of visible selection handles (mobile).
    pub fn update_selection_handles(&mut self, start: Vec2d, end: Vec2d) {
        self.platform_ops
            .push(CxOsOp::UpdateSelectionHandles { start, end });
    }

    /// Hide selection handles (mobile).
    pub fn hide_selection_handles(&mut self) {
        self.platform_ops.push(CxOsOp::HideSelectionHandles);
    }

    pub fn start_dragging(&mut self, items: Vec<DragItem>) {
        self.platform_ops.iter().for_each(|p| {
            if let CxOsOp::StartDragging { .. } = p {
                panic!("start drag twice");
            }
        });
        self.platform_ops.push(CxOsOp::StartDragging(items));
    }

    pub fn set_cursor(&mut self, cursor: MouseCursor) {
        // down cursor overrides the hover cursor
        if let Some(p) = self.platform_ops.iter_mut().find(|p| match p {
            CxOsOp::SetCursor(_) => true,
            _ => false,
        }) {
            *p = CxOsOp::SetCursor(cursor)
        } else {
            self.platform_ops.push(CxOsOp::SetCursor(cursor))
        }
    }

    pub fn sweep_lock(&mut self, value: Area) {
        self.fingers.sweep_lock(value);
    }

    pub fn sweep_unlock(&mut self, value: Area) {
        self.fingers.sweep_unlock(value);
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
    /// ***NOTE***: this must be re-invoked every time the area changes, which is upon every draw pass.
    ///
    /// If you want to block scrolling everywhere, pass in `Area::Empty`.
    pub fn block_scrolling_except_within(&mut self, scrollable_area: Area) {
        self.fingers
            .block_scrolling_within_area(Some(scrollable_area));
    }

    /// Fully unblocks scrolling, allowing scrolling to occur anywhere across the entire app.
    ///
    /// This effectively restores the default behavior, e.g., after a previous call to
    /// [`Cx::block_scrolling_except_within()`].
    pub fn unblock_scrolling(&mut self) {
        self.fingers.block_scrolling_within_area(None);
    }

    pub fn start_timeout(&mut self, delay: f64) -> Timer {
        self.timer_id += 1;
        self.platform_ops.push(CxOsOp::StartTimer {
            timer_id: self.timer_id,
            interval: delay,
            repeats: false,
        });
        Timer(self.timer_id)
    }

    pub fn start_interval(&mut self, interval: f64) -> Timer {
        self.timer_id += 1;
        self.platform_ops.push(CxOsOp::StartTimer {
            timer_id: self.timer_id,
            interval,
            repeats: true,
        });
        Timer(self.timer_id)
    }

    pub fn stop_timer(&mut self, timer: Timer) {
        if timer.0 != 0 {
            self.platform_ops.push(CxOsOp::StopTimer(timer.0));
        }
    }

    pub fn request_permission(&mut self, permission: crate::permission::Permission) -> i32 {
        self.permissions_request_id += 1;
        self.platform_ops.push(CxOsOp::RequestPermission {
            request_id: self.permissions_request_id,
            permission,
        });
        self.permissions_request_id
    }

    pub fn get_dpi_factor_of(&mut self, area: &Area) -> f64 {
        if let Some(draw_list_id) = area.draw_list_id() {
            let draw_pass_id = self.draw_lists[draw_list_id].draw_pass_id.unwrap();
            return self.get_delegated_dpi_factor(draw_pass_id);
        }
        return 1.0;
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

    pub fn redraw_pass_and_parent_passes(&mut self, draw_pass_id: DrawPassId) {
        let mut walk_pass_id = draw_pass_id;
        loop {
            if let Some(main_list_id) = self.passes[walk_pass_id].main_draw_list_id {
                self.redraw_list_and_children(main_list_id);
            }
            match self.passes[walk_pass_id].parent.clone() {
                CxDrawPassParent::DrawPass(next_pass_id) => {
                    walk_pass_id = next_pass_id;
                }
                _ => {
                    break;
                }
            }
        }
    }

    pub fn get_pass_rect(&self, draw_pass_id: DrawPassId, dpi: f64) -> Option<Rect> {
        match self.passes[draw_pass_id].pass_rect {
            Some(CxDrawPassRect::Area(area)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: (rect.pos * dpi).floor() / dpi,
                    size: (rect.size * dpi).ceil() / dpi,
                })
            }
            Some(CxDrawPassRect::AreaOrigin(area, origin)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: origin,
                    size: (rect.size * dpi).ceil() / dpi,
                })
            }
            /*Some(CxDrawPassRect::ScaledArea(area, scale)) => {
                let rect = area.rect(self);
                Some(Rect {
                    pos: (rect.pos * dpi).floor() / dpi,
                    size: scale * (rect.size * dpi).ceil() / dpi,
                })
            }*/
            Some(CxDrawPassRect::Size(size)) => Some(Rect {
                pos: Vec2d::default(),
                size: (size * dpi).ceil() / dpi,
            }),
            None => None,
        }
    }

    pub fn get_pass_name(&self, draw_pass_id: DrawPassId) -> &str {
        &self.passes[draw_pass_id].debug_name
    }

    pub fn repaint_pass(&mut self, draw_pass_id: DrawPassId) {
        let cxpass = &mut self.passes[draw_pass_id];
        cxpass.paint_dirty = true;
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

    pub fn redraw_all(&mut self) {
        self.new_draw_event.redraw_all = true;
    }

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

    pub fn redraw_area_and_children(&mut self, area: Area) {
        if let Some(draw_list_id) = area.draw_list_id() {
            self.redraw_list_and_children(draw_list_id);
        }
    }

    pub fn redraw_list(&mut self, draw_list_id: DrawListId) {
        if self.in_draw_event {
            return;
        }
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

    pub fn redraw_list_and_children(&mut self, draw_list_id: DrawListId) {
        if self.in_draw_event {
            return;
        }
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

    pub fn key_focus(&self) -> Area {
        self.keyboard.key_focus()
    }

    pub fn revert_key_focus(&mut self) {
        self.keyboard.revert_key_focus();
    }

    pub fn has_key_focus(&self, focus_area: Area) -> bool {
        self.keyboard.has_key_focus(focus_area)
    }

    pub fn new_next_frame(&mut self) -> NextFrame {
        let res = NextFrame(self.next_frame_id);
        self.next_frame_id += 1;
        self.new_next_frames.insert(res);
        res
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
            self.platform_ops.push(CxOsOp::WebSocketOpen{
                request,
                request_id,
            });
        }

        pub fn web_socket_send_binary(&mut self, request_id: LiveId, data: Vec<u8>) {
            self.platform_ops.push(CxOsOp::WebSocketSendBinary{
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
        if let VideoSource::Camera(..) = &source {
            // Auto-request camera permission before opening device
            let _request_id = self.request_permission(crate::permission::Permission::Camera);
            self.pending_camera_playbacks.push(crate::cx::PendingCameraPlayback {
                video_id,
                source,
                camera_preview_mode,
                external_texture_id,
                texture_id,
                autoplay,
                should_loop,
            });
            return;
        }
        self.platform_ops.push(CxOsOp::PrepareVideoPlayback(
            video_id,
            source,
            camera_preview_mode,
            external_texture_id,
            texture_id,
            autoplay,
            should_loop,
        ));
    }

    pub fn handle_camera_permission_result(&mut self, result: &crate::permission::PermissionResult) {
        if result.permission != crate::permission::Permission::Camera {
            return;
        }
        let pending: Vec<_> = self.pending_camera_playbacks.drain(..).collect();
        for p in pending {
            match result.status {
                crate::permission::PermissionStatus::Granted => {
                    self.platform_ops.push(CxOsOp::PrepareVideoPlayback(
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
                            error: "Camera permission denied".to_string(),
                        },
                    ));
                }
            }
        }
    }

    pub fn attach_camera_native_preview(&mut self, video_id: LiveId, area: Area) {
        self.platform_ops
            .push(CxOsOp::AttachCameraNativePreview { video_id, area });
    }

    pub fn update_camera_native_preview(&mut self, video_id: LiveId, area: Area, visible: bool) {
        self.platform_ops.push(CxOsOp::UpdateCameraNativePreview {
            video_id,
            area,
            visible,
        });
    }

    pub fn detach_camera_native_preview(&mut self, video_id: LiveId) {
        self.platform_ops
            .push(CxOsOp::DetachCameraNativePreview { video_id });
    }

    pub fn begin_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops.push(CxOsOp::BeginVideoPlayback(video_id));
    }

    pub fn pause_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops.push(CxOsOp::PauseVideoPlayback(video_id));
    }

    pub fn resume_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops
            .push(CxOsOp::ResumeVideoPlayback(video_id));
    }

    pub fn mute_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops.push(CxOsOp::MuteVideoPlayback(video_id));
    }

    pub fn unmute_video_playback(&mut self, video_id: LiveId) {
        self.platform_ops
            .push(CxOsOp::UnmuteVideoPlayback(video_id));
    }

    pub fn cleanup_video_playback_resources(&mut self, video_id: LiveId) {
        self.platform_ops
            .push(CxOsOp::CleanupVideoPlaybackResources(video_id));
    }

    pub fn seek_video_playback(&mut self, video_id: LiveId, position_ms: u64) {
        self.platform_ops
            .push(CxOsOp::SeekVideoPlayback(video_id, position_ms));
    }

    pub fn set_video_volume(&mut self, video_id: LiveId, volume: f64) {
        self.platform_ops
            .push(CxOsOp::SetVideoVolume(video_id, volume));
    }

    pub fn set_video_playback_rate(&mut self, video_id: LiveId, rate: f64) {
        self.platform_ops
            .push(CxOsOp::SetVideoPlaybackRate(video_id, rate));
    }

    pub fn prepare_audio_playback(
        &mut self,
        video_id: LiveId,
        source: VideoSource,
        autoplay: bool,
        should_loop: bool,
    ) {
        self.platform_ops.push(CxOsOp::PrepareAudioPlayback(
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
        self.platform_ops
            .push(CxOsOp::SaveFileDialog(FileDialog::new()));
    }

    pub fn open_system_openfile_dialog(&mut self) {
        self.platform_ops
            .push(CxOsOp::SelectFileDialog(FileDialog::new()));
    }

    pub fn open_system_savefolder_dialog(&mut self) {
        self.platform_ops
            .push(CxOsOp::SaveFolderDialog(FileDialog::new()));
    }

    pub fn open_system_openfolder_dialog(&mut self) {
        self.platform_ops
            .push(CxOsOp::SelectFolderDialog(FileDialog::new()));
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

#[cfg(all(target_os = "linux", not(target_os = "android")))]
fn can_play_type_impl(mime: &str) -> &'static str {
    crate::os::linux::linux_video_playback::can_play_type(mime)
}

#[cfg(target_os = "android")]
fn can_play_type_impl(mime: &str) -> &'static str {
    crate::os::linux::android::android_video_playback::can_play_type(mime)
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
fn can_play_type_impl(mime: &str) -> &'static str {
    crate::os::apple::apple_video_playback::can_play_type(mime)
}

#[cfg(target_os = "windows")]
fn can_play_type_impl(mime: &str) -> &'static str {
    crate::os::windows::windows_video_playback::WindowsVideoPlayer::can_play_type(mime)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "windows",
)))]
fn can_play_type_impl(_mime: &str) -> &'static str {
    ""
}

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
