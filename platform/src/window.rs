use crate::{
    cx::Cx,
    cx_api::CxOsOp,
    draw_pass::{CxDrawPassParent, DrawPass, DrawPassId},
    event::{SafeAreaInsets, VirtualKeyboardEvent, WindowGeom},
    id_pool::*,
    makepad_error_log::*,
    makepad_math::*,
    //makepad_live_id::*,
    makepad_script::*,
    screen::{sanitize_window_geom, DEFAULT_WINDOW_SIZE},
    script::vm::*,
};

#[cfg(any(target_arch = "wasm32", test))]
use crate::event::{Event, WindowGeomChangeEvent};

pub struct WindowHandle(PoolId);

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct WindowId(pub usize, pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook, Default)]
pub enum WindowBackdrop {
    #[default]
    None,
    Auto,
    Mica,
    Acrylic,
    Vibrancy,
    Blur,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook, Default)]
pub enum MacosWindowKind {
    #[default]
    Standard,
    FloatingPanel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook, Default)]
pub enum MacosWindowChrome {
    #[default]
    Titled,
    Borderless,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook, Default)]
pub enum MacosWindowLevel {
    #[default]
    Normal,
    Floating,
    StatusBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script)]
pub struct MacosWindowConfig {
    #[live]
    pub kind: MacosWindowKind,
    #[live]
    pub chrome: MacosWindowChrome,
    #[live]
    pub level: MacosWindowLevel,
    #[live]
    pub non_activating: bool,
    #[live]
    pub closable: bool,
    #[live]
    pub miniaturizable: bool,
    #[live]
    pub resizable: bool,
    #[live]
    pub join_all_spaces: bool,
    #[live]
    pub full_screen_auxiliary: bool,
    #[live]
    pub becomes_key_only_if_needed: bool,
}

impl Default for MacosWindowConfig {
    fn default() -> Self {
        Self {
            kind: MacosWindowKind::Standard,
            chrome: MacosWindowChrome::Titled,
            level: MacosWindowLevel::Normal,
            non_activating: false,
            closable: true,
            miniaturizable: true,
            resizable: true,
            join_all_spaces: false,
            full_screen_auxiliary: false,
            becomes_key_only_if_needed: false,
        }
    }
}

impl MacosWindowConfig {
    pub fn floating_panel() -> Self {
        Self {
            kind: MacosWindowKind::FloatingPanel,
            chrome: MacosWindowChrome::Titled,
            level: MacosWindowLevel::Floating,
            non_activating: true,
            closable: true,
            miniaturizable: false,
            resizable: false,
            join_all_spaces: true,
            full_screen_auxiliary: true,
            becomes_key_only_if_needed: false,
        }
    }

    pub fn normalized(self) -> Self {
        self
    }
}

impl ScriptHook for MacosWindowConfig {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if self.kind != MacosWindowKind::FloatingPanel {
            return;
        }

        let mut has_level = false;
        let mut has_non_activating = false;
        let mut has_miniaturizable = false;
        let mut has_resizable = false;
        let mut has_join_all_spaces = false;
        let mut has_full_screen_auxiliary = false;

        if let Some(obj) = value.as_object() {
            vm.map_mut_with(obj, |_vm, map| {
                for (key, _) in map.iter() {
                    match key.as_id() {
                        Some(id!(level)) => has_level = true,
                        Some(id!(non_activating)) => has_non_activating = true,
                        Some(id!(miniaturizable)) => has_miniaturizable = true,
                        Some(id!(resizable)) => has_resizable = true,
                        Some(id!(join_all_spaces)) => has_join_all_spaces = true,
                        Some(id!(full_screen_auxiliary)) => has_full_screen_auxiliary = true,
                        _ => {}
                    }
                }
            });
        }

        if !has_level {
            self.level = MacosWindowLevel::Floating;
        }
        if !has_non_activating {
            self.non_activating = true;
        }
        if !has_miniaturizable {
            self.miniaturizable = false;
        }
        if !has_resizable {
            self.resizable = false;
        }
        if !has_join_all_spaces {
            self.join_all_spaces = true;
        }
        if !has_full_screen_auxiliary {
            self.full_screen_auxiliary = true;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowVisuals {
    pub transparent: bool,
    pub backdrop: WindowBackdrop,
    pub backdrop_intensity: f32,
}

impl Default for WindowVisuals {
    fn default() -> Self {
        Self {
            transparent: false,
            backdrop: WindowBackdrop::None,
            backdrop_intensity: 1.0,
        }
    }
}

impl WindowVisuals {
    pub fn normalized(mut self) -> Self {
        self.backdrop_intensity = self.backdrop_intensity.clamp(0.0, 1.0);
        self
    }
}

impl WindowId {
    pub fn id(&self) -> usize {
        self.0
    }
}

impl WindowHandle {
    pub fn window_id(&self) -> WindowId {
        WindowId(self.0.id, self.0.generation)
    }
}

#[derive(Default)]
pub struct CxWindowPool(IdPool<CxWindow>);
impl CxWindowPool {
    fn alloc(&mut self) -> WindowHandle {
        WindowHandle(self.0.alloc())
    }

    pub fn len(&self) -> usize {
        self.0.pool.len()
    }

    pub fn window_id_contains(&self, pos: Vec2d) -> (WindowId, Vec2d) {
        for (index, item) in self.0.pool.iter().enumerate() {
            let window = &item.item;
            if pos.x >= window.window_geom.position.x
                && pos.y >= window.window_geom.position.y
                && pos.x <= window.window_geom.position.x + window.window_geom.inner_size.x
                && pos.y <= window.window_geom.position.y + window.window_geom.inner_size.y
            {
                return (
                    WindowId(index, item.generation),
                    window.window_geom.position,
                );
            }
        }
        return (
            WindowId(0, self.0.pool[0].generation),
            self.0.pool[0].item.window_geom.position,
        );
    }

    pub fn relative_to_window_id(&self, pos: Vec2d) -> (WindowId, Vec2d) {
        for (index, item) in self.0.pool.iter().enumerate() {
            let window = &item.item;
            if pos.x >= window.window_geom.position.x
                && pos.y >= window.window_geom.position.y
                && pos.x <= window.window_geom.position.x + window.window_geom.inner_size.x
                && pos.y <= window.window_geom.position.x + window.window_geom.inner_size.y
            {
                return (
                    WindowId(index, item.generation),
                    window.window_geom.position,
                );
            }
        }
        return (
            WindowId(0, self.0.pool[0].generation),
            self.0.pool[0].item.window_geom.position,
        );
    }

    /// Every allocated window slot, as a generation-correct `WindowId`.
    /// Callers usually want to skip the ones whose `is_created` is false.
    pub fn id_iter(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.0
            .pool
            .iter()
            .enumerate()
            .map(|(index, item)| WindowId(index, item.generation))
    }

    /// Returns physical slot zero with its current generation, if it exists.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn current_id_zero(&self) -> Option<WindowId> {
        self.id_iter().next()
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn web_resize_window_geom(
        &mut self,
        cached_native_geom: &mut WindowGeom,
        cached_window_zero_geom: &mut WindowGeom,
        new_native_geom: WindowGeom,
    ) -> Option<WindowGeomChangeEvent> {
        *cached_native_geom = new_native_geom.clone();
        let Some(window_id) = self.current_id_zero() else {
            *cached_window_zero_geom = new_native_geom;
            return None;
        };

        let window = &mut self[window_id];
        let old_geom = window.window_geom.clone();
        window.os_dpi_factor = Some(new_native_geom.dpi_factor);
        let new_geom = window.native_window_geom_to_layout(new_native_geom);
        if old_geom == new_geom {
            return None;
        }

        *cached_window_zero_geom = new_geom.clone();
        window.window_geom = new_geom.clone();
        Some(WindowGeomChangeEvent {
            window_id,
            old_geom,
            new_geom,
        })
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn web_create_window_geom(
        &mut self,
        window_id: WindowId,
        cached_native_geom: &WindowGeom,
        cached_window_zero_geom: &mut WindowGeom,
    ) -> WindowGeomChangeEvent {
        let window = &mut self[window_id];
        window.os_dpi_factor = Some(cached_native_geom.dpi_factor);
        let new_geom = window.native_window_geom_to_layout(cached_native_geom.clone());
        window.window_geom = new_geom.clone();
        if window_id.0 == 0 {
            *cached_window_zero_geom = new_geom.clone();
        }
        WindowGeomChangeEvent {
            window_id,
            old_geom: new_geom.clone(),
            new_geom,
        }
    }

    pub fn is_valid(&self, v: WindowId) -> bool {
        if v.0 < self.0.pool.len() {
            if self.0.pool[v.0].generation == v.1 {
                return true;
            }
        }
        false
    }

    pub fn id_zero() -> WindowId {
        WindowId(0, 0)
    }

    pub fn from_usize(v: usize) -> WindowId {
        WindowId(v, 0)
    }
}

#[cfg(any(target_arch = "wasm32", test))]
impl Cx {
    pub(crate) fn call_window_zero_focus_event(&mut self, focused: bool) {
        let window_id = self
            .windows
            .current_id_zero()
            .unwrap_or_else(CxWindowPool::id_zero);
        let event = if focused {
            Event::WindowGotFocus(window_id)
        } else {
            Event::WindowLostFocus(window_id)
        };
        self.call_event_handler(&event);
    }
}

impl std::ops::Index<WindowId> for CxWindowPool {
    type Output = CxWindow;
    fn index(&self, index: WindowId) -> &Self::Output {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "Window id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &d.item
    }
}

impl std::ops::IndexMut<WindowId> for CxWindowPool {
    fn index_mut(&mut self, index: WindowId) -> &mut Self::Output {
        let d = &mut self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "Window id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &mut d.item
    }
}

impl ScriptHook for WindowHandle {}
impl ScriptNew for WindowHandle {
    fn script_new(vm: &mut ScriptVm) -> Self {
        Self::new(vm.cx_mut())
    }
}
impl ScriptApply for WindowHandle {
    fn script_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
    }
}

/// Linux desktops match a window to its `.desktop` file by app id, and packagers
/// name that file after the binary, so that's the best default we have.
pub(crate) fn default_app_id() -> String {
    std::env::args_os()
        .next()
        .as_ref()
        .and_then(|arg0| std::path::Path::new(arg0).file_name())
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "Makepad".to_string())
}

impl WindowHandle {
    pub fn new(cx: &mut Cx) -> Self {
        let window = cx.windows.alloc();
        let cxwindow = &mut cx.windows[window.window_id()];
        cxwindow.is_created = false;
        cxwindow.create_title = crate::remote::tag_window_title("Makepad".to_string());
        cxwindow.create_inner_size = None;
        cxwindow.create_position = None;
        cxwindow.create_app_id = default_app_id();
        cxwindow.is_popup = false;
        cxwindow.popup_parent = None;
        cxwindow.popup_position = None;
        cxwindow.popup_size = None;
        cxwindow.popup_grab_keyboard = true;
        cx.platform_ops
            .push_back(CxOsOp::CreateWindow(window.window_id()));
        window
    }

    /// Creates a popup window that must be explicitly closed by the app.
    ///
    /// The framework sends `Event::PopupDismissed` when the popup should be
    /// dismissed (outside click, focus loss, Escape). The app must handle that
    /// event and call `close()` on the window handle. The popup is **not**
    /// auto-closed by the framework.
    pub fn new_popup(cx: &mut Cx, parent: WindowId, position: Vec2d, size: Vec2d) -> Self {
        let window = cx.windows.alloc();
        let window_id = window.window_id();
        let grab_keyboard = {
            let cxwindow = &mut cx.windows[window_id];
            cxwindow.is_created = false;
            cxwindow.create_title = "Makepad Popup".to_string();
            cxwindow.create_inner_size = Some(size);
            cxwindow.create_position = Some(position);
            cxwindow.create_app_id = default_app_id();
            cxwindow.is_popup = true;
            cxwindow.popup_parent = Some(parent);
            cxwindow.popup_position = Some(position);
            cxwindow.popup_size = Some(size);
            cxwindow.popup_grab_keyboard = true;
            cxwindow.popup_grab_keyboard
        };
        cx.platform_ops.push_back(CxOsOp::CreatePopupWindow {
            window_id,
            parent_window_id: parent,
            position,
            size,
            grab_keyboard,
        });
        window
    }
}

#[derive(Script)]
pub struct ScriptWindowHandle {
    #[rust(WindowHandle::new(vm.cx_mut()))]
    pub handle: WindowHandle,
    #[live]
    pub title: String,
    /// Wayland `app_id` and X11 WM_CLASS; must match the installed `.desktop`
    /// file's basename or desktops can't find the app's icon. Defaults to argv[0].
    #[live]
    pub app_id: String,
    #[live]
    pub inner_size: Option<Vec2d>,
    #[live]
    pub position: Option<Vec2d>,
    #[live]
    pub kind_id: usize,
    #[live]
    pub dpi_override: Option<f64>,
    #[live]
    pub topmost: bool,
    #[live]
    pub transparent: bool,
    #[live(WindowBackdrop::None)]
    pub backdrop: WindowBackdrop,
    #[live(1.0)]
    pub backdrop_intensity: f32,
    #[live(MacosWindowConfig::default())]
    pub macos: MacosWindowConfig,
    /// Optionally override the caption bar height.
    /// * If `None` (the default), the caption bar's height is based on a system-calculated height
    ///   derived from window chrome button geometry, which will make the window chrome buttons
    ///   nicely vertically centered within the caption bar on all platforms.
    /// * If `Some(value)`, the caption bar height is overridden with the specified fixed value.
    #[live]
    pub caption_bar_height_override: Option<f64>,
}

impl std::ops::Deref for ScriptWindowHandle {
    type Target = WindowHandle;
    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

impl ScriptHook for ScriptWindowHandle {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        let cx = vm.host.cx_mut();
        let window_id = self.handle.window_id();
        if !self.title.is_empty() {
            self.handle.set_title(cx, self.title.clone());
        }
        if !self.app_id.is_empty() {
            cx.windows[window_id].create_app_id = self.app_id.clone();
        }
        if self.inner_size.is_some() {
            cx.windows[window_id].create_inner_size = self.inner_size;
        }
        if self.position.is_some() {
            cx.windows[window_id].create_position = self.position;
        }
        cx.windows[window_id].kind_id = self.kind_id;
        if self.dpi_override.is_some() {
            cx.windows[window_id].dpi_override = self.dpi_override;
        }
        let visuals = WindowVisuals {
            transparent: self.transparent,
            backdrop: self.backdrop,
            backdrop_intensity: self.backdrop_intensity,
        }
        .normalized();
        let macos = self.macos.normalized();
        cx.windows[window_id].macos = macos;
        if cx.windows[window_id].window_visuals() != visuals {
            cx.windows[window_id].transparent = visuals.transparent;
            cx.windows[window_id].backdrop = visuals.backdrop;
            cx.windows[window_id].backdrop_intensity = visuals.backdrop_intensity;
            if cx.windows[window_id].is_created {
                cx.push_unique_platform_op(CxOsOp::SetWindowVisuals(window_id, visuals));
            }
        }
        if self.topmost
            && !(matches!(cx.os_type(), crate::cx::OsType::Macos)
                && macos.level != MacosWindowLevel::Normal)
        {
            self.handle.set_topmost(cx, self.topmost);
        }
    }
}

impl WindowHandle {
    pub fn set_title(&self, cx: &mut Cx, title: String) {
        let title = crate::remote::tag_window_title(title);
        let window_id = self.window_id();
        if cx.windows[window_id].create_title == title {
            return;
        }
        cx.windows[window_id].create_title = title.clone();
        if cx.windows[window_id].is_created {
            cx.push_unique_platform_op(CxOsOp::SetWindowTitle(window_id, title));
        }
    }

    pub fn set_pass(&self, cx: &mut Cx, pass: &DrawPass) {
        cx.windows[self.window_id()].main_pass_id = Some(pass.draw_pass_id());
        cx.passes[pass.draw_pass_id()].parent = CxDrawPassParent::Window(self.window_id());
    }
    pub fn configure_window(
        &mut self,
        cx: &mut Cx,
        inner_size: Vec2d,
        position: Vec2d,
        is_fullscreen: bool,
        title: String,
    ) {
        let window = &mut cx.windows[self.window_id()];
        window.create_title = crate::remote::tag_window_title(title);
        window.create_position = Some(position);
        window.create_inner_size = Some(inner_size);
        window.is_fullscreen = is_fullscreen;
    }
    pub fn configure_macos_window(&mut self, cx: &mut Cx, config: MacosWindowConfig) {
        cx.windows[self.window_id()].macos = config.normalized();
    }
    pub fn get_inner_size(&self, cx: &Cx) -> Vec2d {
        cx.windows[self.window_id()].get_inner_size()
    }

    /// The window's top-left corner, in the space [`Self::reposition`] accepts: physical
    /// screen pixels on Windows and X11, points on macOS. Never scaled by the DPI factor.
    pub fn get_position(&self, cx: &Cx) -> Vec2d {
        cx.windows[self.window_id()].get_position()
    }

    pub fn is_popup(&self, cx: &Cx) -> bool {
        cx.windows[self.window_id()].is_popup
    }

    pub fn set_kind_id(&mut self, cx: &mut Cx, kind_id: usize) {
        cx.windows[self.window_id()].kind_id = kind_id;
    }

    pub fn minimize(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxOsOp::MinimizeWindow(self.window_id()));
    }

    pub fn maximize(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxOsOp::MaximizeWindow(self.window_id()));
    }

    pub fn fullscreen(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxOsOp::FullscreenWindow(self.window_id()));
    }

    pub fn normal(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxOsOp::NormalizeWindow(self.window_id()));
    }

    pub fn can_fullscreen(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id()].window_geom.can_fullscreen
    }

    pub fn is_fullscreen(&self, cx: &Cx) -> bool {
        cx.windows[self.window_id()].window_geom.is_fullscreen
    }

    pub fn xr_is_presenting(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id()].window_geom.xr_is_presenting
    }

    pub fn is_topmost(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id()].window_geom.is_topmost
    }

    pub fn set_topmost(&mut self, cx: &mut Cx, set_topmost: bool) {
        cx.push_unique_platform_op(CxOsOp::SetTopmost(self.window_id(), set_topmost));
    }

    /// Windows only: a maximized window normally keeps a border/titlebar
    /// strip so its client area matches the monitor work area. Setting this
    /// drops that strip while maximized instead, so the window reads as a
    /// clean fullscreen picture (a projector output, say) rather than a
    /// decorated window pinned to the screen. Other backends ignore it.
    pub fn set_chromeless_when_maximized(&mut self, cx: &mut Cx, chromeless: bool) {
        cx.push_unique_platform_op(CxOsOp::SetChromelessWhenMaximized(
            self.window_id(),
            chromeless,
        ));
    }

    pub fn set_window_visuals(&mut self, cx: &mut Cx, visuals: WindowVisuals) {
        let visuals = visuals.normalized();
        let window_id = self.window_id();
        if cx.windows[window_id].window_visuals() != visuals {
            cx.windows[window_id].transparent = visuals.transparent;
            cx.windows[window_id].backdrop = visuals.backdrop;
            cx.windows[window_id].backdrop_intensity = visuals.backdrop_intensity;
            if cx.windows[window_id].is_created {
                cx.push_unique_platform_op(CxOsOp::SetWindowVisuals(window_id, visuals));
            }
        }
    }

    pub fn set_transparent(&mut self, cx: &mut Cx, transparent: bool) {
        let mut visuals = cx.windows[self.window_id()].window_visuals();
        visuals.transparent = transparent;
        self.set_window_visuals(cx, visuals);
    }

    pub fn set_backdrop(&mut self, cx: &mut Cx, backdrop: WindowBackdrop) {
        let mut visuals = cx.windows[self.window_id()].window_visuals();
        visuals.backdrop = backdrop;
        self.set_window_visuals(cx, visuals);
    }

    pub fn set_backdrop_intensity(&mut self, cx: &mut Cx, backdrop_intensity: f32) {
        let mut visuals = cx.windows[self.window_id()].window_visuals();
        visuals.backdrop_intensity = backdrop_intensity;
        self.set_window_visuals(cx, visuals);
    }

    pub fn resize(&self, cx: &mut Cx, size: Vec2d) {
        cx.push_unique_platform_op(CxOsOp::ResizeWindow(self.window_id(), size));
    }

    /// Moves the window's top-left corner to `position`, in the same space
    /// [`Self::get_position`] reports: physical screen pixels on Windows and X11, points on
    /// macOS. Unlike [`Self::resize`], which takes a logical size that scales with the DPI, a
    /// position is never scaled — a screen coordinate spanning displays of different scales
    /// has no single factor to be logical in. Backends fit the request to the displays that
    /// are actually attached, so a window cannot be placed where it could not be reached.
    pub fn reposition(&self, cx: &mut Cx, position: Vec2d) {
        cx.push_unique_platform_op(CxOsOp::RepositionWindow(self.window_id(), position));
    }

    pub fn restore(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxOsOp::RestoreWindow(self.window_id()));
    }

    pub fn close(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxOsOp::CloseWindow(self.window_id()));
    }
}

/// A single RGBA8 pixel buffer for a window icon. Must be square.
#[derive(Clone, Debug)]
pub struct WindowIconBuffer {
    pub width: u32,
    pub height: u32,
    pub scale: i32,
    /// Row-major RGBA8 pixel data. Length must be `width * height * 4`.
    pub data: Vec<u8>,
}

/// Window icon descriptor with optional name and one or more pixel buffers.
#[derive(Clone, Debug, Default)]
pub struct WindowIcon {
    /// Optional human-readable name (used as Wayland `app_id` when set).
    pub name: Option<String>,
    /// Pixel buffers at various sizes/scales.
    pub buffers: Vec<WindowIconBuffer>,
}

#[derive(Clone)]
pub struct CxWindow {
    pub create_title: String,
    pub create_position: Option<Vec2d>,
    pub create_inner_size: Option<Vec2d>,
    pub create_icon: Option<WindowIcon>,
    pub create_app_id: String,
    pub kind_id: usize,
    pub dpi_override: Option<f64>,
    pub os_dpi_factor: Option<f64>,
    pub is_created: bool,
    pub window_geom: WindowGeom,
    pub main_pass_id: Option<DrawPassId>,
    pub is_fullscreen: bool,
    pub is_popup: bool,
    pub popup_parent: Option<WindowId>,
    pub popup_position: Option<Vec2d>,
    pub popup_size: Option<Vec2d>,
    pub popup_grab_keyboard: bool,
    pub transparent: bool,
    pub backdrop: WindowBackdrop,
    pub backdrop_intensity: f32,
    pub macos: MacosWindowConfig,
}

impl Default for CxWindow {
    fn default() -> Self {
        Self {
            create_title: String::default(),
            create_position: None,
            create_inner_size: None,
            create_icon: None,
            create_app_id: String::default(),
            kind_id: 0,
            dpi_override: None,
            os_dpi_factor: None,
            is_created: false,
            window_geom: WindowGeom::default(),
            main_pass_id: None,
            is_fullscreen: false,
            is_popup: false,
            popup_parent: None,
            popup_position: None,
            popup_size: None,
            popup_grab_keyboard: true,
            transparent: false,
            backdrop: WindowBackdrop::None,
            backdrop_intensity: 1.0,
            macos: MacosWindowConfig::default(),
        }
    }
}

impl CxWindow {
    /// The geometry to create this window with, reduced to values a windowing system can act
    /// on: a size no smaller than [`crate::screen::MIN_WINDOW_SIZE`], and a position that is
    /// either real coordinates or `None` for "the system places it".
    ///
    /// Every backend reads its creation geometry through here, so no request — a restored
    /// state file, a DSL literal, a computed popup rect — can reach a platform call carrying a
    /// size it will reject or a coordinate that is not a number. Placing the window on a
    /// display that exists is a separate, per-backend step; see
    /// [`crate::screen::fit_window_rect_to_screens`].
    pub fn create_geom(&self) -> (Option<Vec2d>, Vec2d) {
        sanitize_window_geom(
            self.create_position,
            self.create_inner_size.unwrap_or(DEFAULT_WINDOW_SIZE),
        )
    }

    pub(crate) fn valid_dpi_factor(dpi_factor: f64) -> Option<f64> {
        if dpi_factor.is_finite() && dpi_factor > 0.0 {
            Some(dpi_factor)
        } else {
            None
        }
    }

    pub(crate) fn scale_rect(mut rect: Rect, scale: f64) -> Rect {
        rect.pos *= scale;
        rect.size *= scale;
        rect
    }

    pub fn window_visuals(&self) -> WindowVisuals {
        WindowVisuals {
            transparent: self.transparent,
            backdrop: self.backdrop,
            backdrop_intensity: self.backdrop_intensity,
        }
        .normalized()
    }

    /// Native OS scale factor reported by the platform before `dpi_override`.
    pub fn native_dpi_factor(&self) -> f64 {
        self.os_dpi_factor
            .and_then(Self::valid_dpi_factor)
            .or_else(|| Self::valid_dpi_factor(self.window_geom.dpi_factor))
            .unwrap_or(1.0)
    }

    /// Effective Makepad layout scale factor after applying `dpi_override`.
    pub fn effective_dpi_factor(&self) -> f64 {
        self.dpi_override
            .and_then(Self::valid_dpi_factor)
            .or_else(|| self.os_dpi_factor.and_then(Self::valid_dpi_factor))
            .or_else(|| Self::valid_dpi_factor(self.window_geom.dpi_factor))
            .unwrap_or(1.0)
    }

    /// Converts native OS logical points into Makepad layout points.
    ///
    /// Use this for values reported in UIKit/AppKit/window-system points, such
    /// as safe-area insets on iOS or OS-native window chrome geometry.
    pub fn native_points_to_layout(&self, value: f64) -> f64 {
        value * self.native_dpi_factor() / self.effective_dpi_factor()
    }

    pub fn native_vec2d_to_layout(&self, value: Vec2d) -> Vec2d {
        value * (self.native_dpi_factor() / self.effective_dpi_factor())
    }

    pub fn native_rect_to_layout(&self, rect: Rect) -> Rect {
        Self::scale_rect(rect, self.native_dpi_factor() / self.effective_dpi_factor())
    }

    pub fn native_safe_area_insets_to_layout(&self, insets: SafeAreaInsets) -> SafeAreaInsets {
        insets.scale(self.native_dpi_factor() / self.effective_dpi_factor())
    }

    /// Converts physical pixels into Makepad layout points.
    ///
    /// Use this for Android surface, touch, keyboard, and overlay values that
    /// arrive from the OS in raw pixels.
    pub fn physical_pixels_to_layout(&self, value: f64) -> f64 {
        value / self.effective_dpi_factor()
    }

    pub fn physical_vec2d_to_layout(&self, value: Vec2d) -> Vec2d {
        value / self.effective_dpi_factor()
    }

    pub fn physical_safe_area_insets_to_layout(&self, insets: SafeAreaInsets) -> SafeAreaInsets {
        insets.scale(1.0 / self.effective_dpi_factor())
    }

    /// Converts Makepad layout points back into native OS logical points.
    pub fn layout_points_to_native_points(&self, value: f64) -> f64 {
        value * self.effective_dpi_factor() / self.native_dpi_factor()
    }

    pub fn layout_vec2d_to_native_points(&self, value: Vec2d) -> Vec2d {
        value * (self.effective_dpi_factor() / self.native_dpi_factor())
    }

    pub fn layout_rect_to_native_points(&self, rect: Rect) -> Rect {
        Self::scale_rect(rect, self.effective_dpi_factor() / self.native_dpi_factor())
    }

    /// Converts Makepad layout points into physical pixels.
    pub fn layout_points_to_physical_pixels(&self, value: f64) -> f64 {
        value * self.effective_dpi_factor()
    }

    pub fn layout_vec2d_to_physical_pixels(&self, value: Vec2d) -> Vec2d {
        value * self.effective_dpi_factor()
    }

    pub fn layout_rect_to_physical_pixels(&self, rect: Rect) -> Rect {
        Self::scale_rect(rect, self.effective_dpi_factor())
    }

    pub fn native_virtual_keyboard_event_to_layout(
        &self,
        event: VirtualKeyboardEvent,
    ) -> VirtualKeyboardEvent {
        match event {
            VirtualKeyboardEvent::WillShow {
                time,
                height,
                duration,
                ease,
            } => VirtualKeyboardEvent::WillShow {
                time,
                height: self.native_points_to_layout(height),
                duration,
                ease,
            },
            VirtualKeyboardEvent::WillHide {
                time,
                height,
                duration,
                ease,
            } => VirtualKeyboardEvent::WillHide {
                time,
                height: self.native_points_to_layout(height),
                duration,
                ease,
            },
            VirtualKeyboardEvent::DidShow { time, height } => VirtualKeyboardEvent::DidShow {
                time,
                height: self.native_points_to_layout(height),
            },
            VirtualKeyboardEvent::DidHide { time } => VirtualKeyboardEvent::DidHide { time },
        }
    }

    /// Converts a `WindowGeom` reported in native OS points into Makepad layout
    /// points, applying any active `dpi_override` to every in-window metric.
    pub fn native_window_geom_to_layout(&self, mut geom: WindowGeom) -> WindowGeom {
        let native_dpi =
            Self::valid_dpi_factor(geom.dpi_factor).unwrap_or(self.native_dpi_factor());
        let effective_dpi =
            Self::valid_dpi_factor(self.dpi_override.unwrap_or(native_dpi)).unwrap_or(native_dpi);
        let scale = native_dpi / effective_dpi;
        geom.inner_size *= scale;
        geom.outer_size *= scale;
        geom.safe_area_insets = geom.safe_area_insets.scale(scale);
        geom.window_chrome_buttons = Self::scale_rect(geom.window_chrome_buttons, scale);
        geom.dpi_factor = effective_dpi;
        geom
    }

    pub fn remap_dpi_override(&self, pos: Vec2d) -> Vec2d {
        self.native_vec2d_to_layout(pos)
    }

    pub fn get_inner_size(&self) -> Vec2d {
        self.window_geom.inner_size
    }

    pub fn get_position(&self) -> Vec2d {
        self.window_geom.position
    }

    /*
    pub fn get_dpi_factor(&mut self) -> Option<f32> {
        if self.is_created {
            Some(self.window_geom.dpi_factor)
        }
        else{
            None
        }
    }*/
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use crate::script::vm::ScriptVmCx;
    use std::{cell::RefCell, rc::Rc};

    fn test_cx() -> Cx {
        Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}))
    }

    fn web_geom(dpi_factor: f64, width: f64, height: f64) -> WindowGeom {
        WindowGeom {
            dpi_factor,
            inner_size: dvec2(width, height),
            outer_size: dvec2(width, height),
            ..Default::default()
        }
    }

    #[test]
    fn web_cached_native_geometry_is_applied_when_window_zero_is_created() {
        let mut windows = CxWindowPool::default();
        let mut native_cache = WindowGeom::default();
        let mut window_zero_cache = WindowGeom::default();
        let native_geom = web_geom(3.0, 400.0, 300.0);

        assert!(windows
            .web_resize_window_geom(
                &mut native_cache,
                &mut window_zero_cache,
                native_geom.clone(),
            )
            .is_none());
        let window_zero = windows.alloc();
        let window_zero_id = window_zero.window_id();
        windows[window_zero_id].dpi_override = Some(2.0);

        let event = windows.web_create_window_geom(
            window_zero_id,
            &native_cache,
            &mut window_zero_cache,
        );

        assert_eq!(native_cache, native_geom);
        assert_eq!(event.window_id, window_zero_id);
        assert_eq!(event.old_geom, event.new_geom);
        assert_eq!(event.new_geom.inner_size, dvec2(600.0, 450.0));
        assert_eq!(windows[window_zero_id].window_geom, event.new_geom);
        assert_eq!(window_zero_cache, event.new_geom);
    }

    #[test]
    fn web_resize_caches_before_creation_then_converts_and_emits_after() {
        let mut windows = CxWindowPool::default();
        let mut native_cache = WindowGeom::default();
        let mut window_zero_cache = WindowGeom::default();
        let first_native = web_geom(2.0, 100.0, 80.0);
        let replacement_native = web_geom(3.0, 120.0, 90.0);

        assert!(windows
            .web_resize_window_geom(
                &mut native_cache,
                &mut window_zero_cache,
                first_native,
            )
            .is_none());
        assert!(windows
            .web_resize_window_geom(
                &mut native_cache,
                &mut window_zero_cache,
                replacement_native.clone(),
            )
            .is_none());
        assert_eq!(native_cache, replacement_native);
        assert_eq!(window_zero_cache, replacement_native);

        let window_zero = windows.alloc();
        let window_zero_id = window_zero.window_id();
        windows[window_zero_id].dpi_override = Some(2.0);
        windows.web_create_window_geom(
            window_zero_id,
            &native_cache,
            &mut window_zero_cache,
        );
        let old_window_geom = windows[window_zero_id].window_geom.clone();
        window_zero_cache = web_geom(9.0, 1.0, 1.0);
        let resized_native = web_geom(4.0, 200.0, 150.0);

        let event = windows
            .web_resize_window_geom(
                &mut native_cache,
                &mut window_zero_cache,
                resized_native.clone(),
            )
            .unwrap();

        assert_eq!(event.window_id, window_zero_id);
        assert_eq!(event.old_geom, old_window_geom);
        assert_eq!(event.new_geom.inner_size, dvec2(400.0, 300.0));
        assert_eq!(native_cache, resized_native);
        assert_eq!(window_zero_cache, event.new_geom);
        assert_eq!(windows[window_zero_id].window_geom, event.new_geom);
    }

    #[test]
    fn web_focus_is_delivered_without_a_window_and_uses_current_generation() {
        let received = Rc::new(RefCell::new(Vec::new()));
        let received_by_handler = received.clone();
        let mut cx = Cx::new(Box::new(move |_cx, event| match event {
            Event::WindowGotFocus(window_id) => {
                received_by_handler.borrow_mut().push((true, *window_id));
            }
            Event::WindowLostFocus(window_id) => {
                received_by_handler
                    .borrow_mut()
                    .push((false, *window_id));
            }
            _ => {}
        }));

        assert!(cx.windows.current_id_zero().is_none());
        cx.call_window_zero_focus_event(true);
        cx.call_window_zero_focus_event(false);

        let first_window_zero = WindowHandle::new(&mut cx);
        drop(first_window_zero);
        let recycled_window_zero = WindowHandle::new(&mut cx);
        assert_eq!(recycled_window_zero.window_id(), WindowId(0, 1));
        cx.call_window_zero_focus_event(true);

        assert_eq!(
            *received.borrow(),
            vec![
                (true, WindowId(0, 0)),
                (false, WindowId(0, 0)),
                (true, recycled_window_zero.window_id()),
            ]
        );
    }

    #[test]
    fn web_nonzero_window_creation_preserves_window_zero_geometry() {
        let mut windows = CxWindowPool::default();
        let mut window_zero_cache = WindowGeom::default();
        let native_cache = web_geom(2.0, 100.0, 80.0);
        let window_zero = windows.alloc();
        let window_zero_id = window_zero.window_id();
        windows[window_zero_id].dpi_override = Some(2.0);
        windows.web_create_window_geom(
            window_zero_id,
            &native_cache,
            &mut window_zero_cache,
        );
        let cached_before = window_zero_cache.clone();
        let window_zero_before = windows[window_zero_id].window_geom.clone();

        let second_window = windows.alloc();
        let second_window_id = second_window.window_id();
        windows[second_window_id].dpi_override = Some(1.0);
        let event = windows.web_create_window_geom(
            second_window_id,
            &native_cache,
            &mut window_zero_cache,
        );

        assert_eq!(event.new_geom.inner_size, dvec2(200.0, 160.0));
        assert_eq!(window_zero_cache, cached_before);
        assert_eq!(windows[window_zero_id].window_geom, window_zero_before);
    }

    #[test]
    fn window_visuals_defaults() {
        let mut cx = test_cx();
        let window = WindowHandle::new(&mut cx);
        let cx_window = &cx.windows[window.window_id()];
        assert!(!cx_window.transparent);
        assert_eq!(cx_window.backdrop, WindowBackdrop::None);
        assert_eq!(cx_window.backdrop_intensity, 1.0);
    }

    #[test]
    fn set_window_visuals_updates_and_dedups_platform_ops() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        let window_id = window.window_id();

        cx.windows[window_id].is_created = true;
        cx.platform_ops.clear();

        window.set_backdrop(&mut cx, WindowBackdrop::Mica);
        assert_eq!(cx.platform_ops.len(), 1);
        assert!(matches!(
            cx.platform_ops[0],
            CxOsOp::SetWindowVisuals(
                _,
                WindowVisuals {
                    backdrop: WindowBackdrop::Mica,
                    ..
                }
            )
        ));

        cx.platform_ops.clear();
        window.set_backdrop(&mut cx, WindowBackdrop::Mica);
        assert!(cx.platform_ops.is_empty());

        window.set_backdrop_intensity(&mut cx, 0.25);
        assert_eq!(cx.platform_ops.len(), 1);
        assert!(matches!(
            cx.platform_ops[0],
            CxOsOp::SetWindowVisuals(
                _,
                WindowVisuals {
                    backdrop_intensity: 0.25,
                    ..
                }
            )
        ));
    }

    #[test]
    fn macos_window_config_default_preserves_standard_window_behavior() {
        assert_eq!(
            MacosWindowConfig::default(),
            MacosWindowConfig {
                kind: MacosWindowKind::Standard,
                chrome: MacosWindowChrome::Titled,
                level: MacosWindowLevel::Normal,
                non_activating: false,
                closable: true,
                miniaturizable: true,
                resizable: true,
                join_all_spaces: false,
                full_screen_auxiliary: false,
                becomes_key_only_if_needed: false,
            }
        );
    }

    #[test]
    fn macos_window_config_floating_panel_preset_matches_plan_defaults() {
        assert_eq!(
            MacosWindowConfig::floating_panel(),
            MacosWindowConfig {
                kind: MacosWindowKind::FloatingPanel,
                chrome: MacosWindowChrome::Titled,
                level: MacosWindowLevel::Floating,
                non_activating: true,
                closable: true,
                miniaturizable: false,
                resizable: false,
                join_all_spaces: true,
                full_screen_auxiliary: true,
                becomes_key_only_if_needed: false,
            }
        );
    }

    #[test]
    fn macos_window_config_script_hook_applies_floating_panel_defaults_only_when_missing() {
        let mut host = test_cx();
        let config = host.with_vm(|vm| {
            let obj = vm.heap_mut().new_object();
            vm.map_mut_with(obj, |_vm, map| {
                map.insert(
                    ScriptValue::from_id(id!(kind)),
                    ScriptMapValue {
                        tag: Default::default(),
                        value: NIL,
                    },
                );
            });

            let mut config = MacosWindowConfig::default();
            config.kind = MacosWindowKind::FloatingPanel;
            config.on_after_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            config
        });

        assert_eq!(config.level, MacosWindowLevel::Floating);
        assert!(config.non_activating);
        assert!(!config.miniaturizable);
        assert!(!config.resizable);
        assert!(config.join_all_spaces);
        assert!(config.full_screen_auxiliary);
    }

    #[test]
    fn configure_macos_window_preserves_explicit_floating_panel_overrides() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        let config = MacosWindowConfig {
            kind: MacosWindowKind::FloatingPanel,
            chrome: MacosWindowChrome::Borderless,
            level: MacosWindowLevel::Normal,
            non_activating: false,
            closable: false,
            miniaturizable: true,
            resizable: true,
            join_all_spaces: false,
            full_screen_auxiliary: false,
            becomes_key_only_if_needed: true,
        };

        window.configure_macos_window(&mut cx, config);

        assert_eq!(cx.windows[window.window_id()].macos, config);
    }

    #[test]
    fn script_window_handle_on_after_apply_writes_macos_config_into_cx_window() {
        let mut host = test_cx();
        host.with_vm(|vm| {
            let handle = WindowHandle::new(vm.cx_mut());
            let window_id = handle.window_id();
            let mut script_window = ScriptWindowHandle {
                handle,
                title: "Floating Panel".to_string(),
                app_id: "floating-panel".to_string(),
                inner_size: Some(dvec2(320.0, 80.0)),
                position: Some(dvec2(40.0, 50.0)),
                kind_id: 7,
                dpi_override: Some(2.0),
                topmost: false,
                transparent: true,
                backdrop: WindowBackdrop::Blur,
                backdrop_intensity: 0.5,
                macos: MacosWindowConfig::floating_panel(),
                caption_bar_height_override: None,
            };

            script_window.on_after_apply(vm, &Apply::New, &mut Scope::empty(), NIL);

            let cx = vm.cx_mut();
            let cx_window = &cx.windows[window_id];
            assert_eq!(cx_window.create_title, "Floating Panel");
            assert_eq!(cx_window.create_app_id, "floating-panel");
            assert_eq!(cx_window.create_inner_size, Some(dvec2(320.0, 80.0)));
            assert_eq!(cx_window.create_position, Some(dvec2(40.0, 50.0)));
            assert_eq!(cx_window.kind_id, 7);
            assert_eq!(cx_window.dpi_override, Some(2.0));
            assert_eq!(
                cx_window.window_visuals(),
                WindowVisuals {
                    transparent: true,
                    backdrop: WindowBackdrop::Blur,
                    backdrop_intensity: 0.5,
                }
            );
            assert_eq!(cx_window.macos, MacosWindowConfig::floating_panel());
        });
    }

    #[test]
    fn dpi_conversion_helpers_keep_native_geometry_physically_fixed() {
        let window = CxWindow {
            dpi_override: Some(2.0),
            os_dpi_factor: Some(3.0),
            window_geom: WindowGeom {
                dpi_factor: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(window.native_points_to_layout(30.0), 45.0);
        assert_eq!(window.physical_pixels_to_layout(90.0), 45.0);
        assert_eq!(window.layout_points_to_native_points(45.0), 30.0);
        assert_eq!(window.layout_points_to_physical_pixels(45.0), 90.0);
        assert_eq!(
            window.native_safe_area_insets_to_layout(SafeAreaInsets {
                top: 30.0,
                right: 10.0,
                bottom: 12.0,
                left: 4.0,
            }),
            SafeAreaInsets {
                top: 45.0,
                right: 15.0,
                bottom: 18.0,
                left: 6.0,
            }
        );
    }

    #[test]
    fn native_window_geom_to_layout_converts_every_in_window_metric() {
        let window = CxWindow {
            dpi_override: Some(2.0),
            os_dpi_factor: Some(3.0),
            ..Default::default()
        };

        let geom = window.native_window_geom_to_layout(WindowGeom {
            dpi_factor: 3.0,
            inner_size: dvec2(400.0, 300.0),
            outer_size: dvec2(420.0, 330.0),
            safe_area_insets: SafeAreaInsets {
                top: 30.0,
                right: 10.0,
                bottom: 12.0,
                left: 4.0,
            },
            window_chrome_buttons: Rect {
                pos: dvec2(8.0, 6.0),
                size: dvec2(72.0, 24.0),
            },
            ..Default::default()
        });

        assert_eq!(geom.dpi_factor, 2.0);
        assert_eq!(geom.inner_size, dvec2(600.0, 450.0));
        assert_eq!(geom.outer_size, dvec2(630.0, 495.0));
        assert_eq!(
            geom.safe_area_insets,
            SafeAreaInsets {
                top: 45.0,
                right: 15.0,
                bottom: 18.0,
                left: 6.0,
            }
        );
        assert_eq!(geom.window_chrome_buttons.pos, dvec2(12.0, 9.0));
        assert_eq!(geom.window_chrome_buttons.size, dvec2(108.0, 36.0));
    }

    #[test]
    fn dpi_factor_helpers_fall_back_past_invalid_stored_values() {
        let window = CxWindow {
            os_dpi_factor: Some(f64::NAN),
            dpi_override: None,
            window_geom: WindowGeom {
                dpi_factor: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(window.native_dpi_factor(), 3.0);
        assert_eq!(window.effective_dpi_factor(), 3.0);

        let window = CxWindow {
            os_dpi_factor: Some(3.0),
            dpi_override: Some(f64::NAN),
            ..Default::default()
        };

        assert_eq!(window.native_dpi_factor(), 3.0);
        assert_eq!(window.effective_dpi_factor(), 3.0);
    }

    #[test]
    fn set_window_dpi_override_converts_every_in_window_metric() {
        let mut cx = test_cx();
        let window = WindowHandle::new(&mut cx);
        let window_id = window.window_id();
        cx.windows[window_id].os_dpi_factor = Some(3.0);
        cx.windows[window_id].window_geom = WindowGeom {
            dpi_factor: 3.0,
            inner_size: dvec2(400.0, 300.0),
            outer_size: dvec2(420.0, 330.0),
            safe_area_insets: SafeAreaInsets {
                top: 30.0,
                right: 10.0,
                bottom: 12.0,
                left: 4.0,
            },
            window_chrome_buttons: Rect {
                pos: dvec2(8.0, 6.0),
                size: dvec2(72.0, 24.0),
            },
            ..Default::default()
        };

        cx.set_window_dpi_override(window_id, Some(2.0));

        let geom = &cx.windows[window_id].window_geom;
        assert_eq!(geom.dpi_factor, 2.0);
        assert_eq!(geom.inner_size, dvec2(600.0, 450.0));
        assert_eq!(geom.outer_size, dvec2(630.0, 495.0));
        assert_eq!(
            geom.safe_area_insets,
            SafeAreaInsets {
                top: 45.0,
                right: 15.0,
                bottom: 18.0,
                left: 6.0,
            }
        );
        assert_eq!(geom.window_chrome_buttons.pos, dvec2(12.0, 9.0));
        assert_eq!(geom.window_chrome_buttons.size, dvec2(108.0, 36.0));
        assert_eq!(cx.pending_window_geom_changes.len(), 1);

        cx.set_window_dpi_override(window_id, None);

        let geom = &cx.windows[window_id].window_geom;
        assert_eq!(geom.dpi_factor, 3.0);
        assert_eq!(geom.inner_size, dvec2(400.0, 300.0));
        assert_eq!(geom.outer_size, dvec2(420.0, 330.0));
        assert_eq!(
            geom.safe_area_insets,
            SafeAreaInsets {
                top: 30.0,
                right: 10.0,
                bottom: 12.0,
                left: 4.0,
            }
        );
        assert_eq!(geom.window_chrome_buttons.pos, dvec2(8.0, 6.0));
        assert_eq!(geom.window_chrome_buttons.size, dvec2(72.0, 24.0));
        assert_eq!(cx.pending_window_geom_changes.len(), 2);
    }

    #[test]
    fn set_topmost_queues_platform_op() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        cx.platform_ops.clear();

        window.set_topmost(&mut cx, true);

        assert_eq!(cx.platform_ops.len(), 1);
        assert!(matches!(cx.platform_ops[0], CxOsOp::SetTopmost(_, true)));
    }

    #[test]
    fn create_window_then_set_topmost_queues_fifo() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        window.set_topmost(&mut cx, true);

        let first = cx.platform_ops.pop_front().unwrap();
        let second = cx.platform_ops.pop_front().unwrap();
        assert!(matches!(first, CxOsOp::CreateWindow(_)));
        assert!(matches!(second, CxOsOp::SetTopmost(_, true)));
        assert!(cx.platform_ops.is_empty());
    }
}
