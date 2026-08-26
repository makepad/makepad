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
    script::vm::*,
};

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

/// One DirectComposition child visual on a `window.direct_composition` window.
///
/// Allocated by [`WindowHandle::dcomp_create_child`]. Windows-only: the
/// matching `CxOsOp` variants are `cfg(target_os = "windows")`.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DcompChildId(pub u64);

/// Z-order of a child visual relative to Makepad's UI swap chain.
///
/// DirectComposition draws a visual's children on top of that visual's own
/// content, so the host tree is a content-less root with the UI as one child.
/// **0 is reserved for that UI visual.** Negative values sit behind it; positive
/// values sit in front. Same-z children are ordered by id (lower id further back).
/// Passing `0` for a child snaps to [`Self::BEHIND`].
///
/// This is not a compositor and not a closed set of roles. The UI is one
/// bitmap; a child can only be in front of or behind that bitmap. Conventional
/// constants below pick the usual slots:
///
/// | Constant | Value | Use |
/// | --- | --- | --- |
/// | [`Self::BACKDROP`] | `i32::MIN` | Opaque fill under every other behind-UI child |
/// | [`Self::BEHIND`] | `-1` | Video / camera underlay |
/// | [`Self::UI`] | `0` | Reserved; not a valid child z |
/// | [`Self::FRONT`] | `1` | Native overlay above the UI |
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DcompChildZ(pub i32);

#[cfg(target_os = "windows")]
impl DcompChildZ {
    /// Opaque fill that must stay under every other behind-UI child, whatever
    /// order they were created in. Pair with [`WindowHandle::dcomp_set_child_solid`]:
    /// a hole punched through the UI cannot stay pixel-exact with a behind-UI
    /// child through a resize (Present vs DComp Commit), so whatever the hole
    /// exposes should land on this instead of the desktop.
    pub const BACKDROP: Self = Self(i32::MIN);
    /// Typical underlay (video, camera), immediately behind the UI.
    pub const BEHIND: Self = Self(-1);
    /// Reserved for the UI swap chain. Not a valid child z.
    pub const UI: Self = Self(0);
    /// Native overlay above Makepad pixels.
    pub const FRONT: Self = Self(1);

    pub fn is_front(self) -> bool {
        self.0 > 0
    }

    /// `0` cannot share the UI's slot.
    pub fn sanitized(self) -> Self {
        if self.0 == 0 {
            Self::BEHIND
        } else {
            self
        }
    }
}

#[cfg(target_os = "windows")]
impl Default for DcompChildZ {
    fn default() -> Self {
        Self::BEHIND
    }
}

/// Placement of a child visual, in **physical pixels** from the window's
/// client origin. Logical Makepad coordinates must be multiplied by DPI first.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DcompChildGeom {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Stretch the child's swap-chain bitmap to `width`×`height`. `1` is 1:1.
    /// Used while live-resizing so an old buffer still covers a punched hole.
    pub scale_x: f32,
    pub scale_y: f32,
    pub visible: bool,
}

#[cfg(target_os = "windows")]
impl Default for DcompChildGeom {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            visible: false,
        }
    }
}

#[cfg(target_os = "windows")]
impl DcompChildGeom {
    pub fn is_shown(&self) -> bool {
        self.visible && self.width > 0.0 && self.height > 0.0
    }

    /// Non-finite offsets/sizes become 0; negative size is clamped.
    pub fn sanitized(self) -> Self {
        fn coord(v: f32) -> f32 {
            if v.is_finite() { v } else { 0.0 }
        }
        fn extent(v: f32) -> f32 {
            if v.is_finite() { v.max(0.0) } else { 0.0 }
        }
        fn scale(v: f32) -> f32 {
            if v.is_finite() && v > 0.0 { v } else { 1.0 }
        }
        Self {
            x: coord(self.x),
            y: coord(self.y),
            width: extent(self.width),
            height: extent(self.height),
            scale_x: scale(self.scale_x),
            scale_y: scale(self.scale_y),
            visible: self.visible,
        }
    }

    /// Layout/DPI rounding can wobble by a fraction of a pixel; skip DWM
    /// commits unless the visual actually moved.
    pub fn approx_eq(self, other: Self) -> bool {
        const EPS: f32 = 0.5;
        const SCALE_EPS: f32 = 0.001;
        self.visible == other.visible
            && (self.x - other.x).abs() <= EPS
            && (self.y - other.y).abs() <= EPS
            && (self.width - other.width).abs() <= EPS
            && (self.height - other.height).abs() <= EPS
            && (self.scale_x - other.scale_x).abs() <= SCALE_EPS
            && (self.scale_y - other.scale_y).abs() <= SCALE_EPS
    }
}

/// COM content for [`IDCompositionVisual::SetContent`](https://learn.microsoft.com/windows/win32/api/dcomp/nf-dcomp-idcompositionvisual-setcontent).
///
/// Typically an `IDXGISwapChain1` from `CreateSwapChainForComposition`. The
/// pointer owns one `IUnknown` reference; dropping this value `Release`s it.
/// Not `Send`/`Sync`: DirectComposition and the COM pointer are UI-thread only.
#[cfg(target_os = "windows")]
pub struct DcompContent {
    ptr: *mut std::ffi::c_void,
    _not_send_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(target_os = "windows")]
impl DcompContent {
    /// Takes ownership of one `IUnknown` reference.
    ///
    /// # Safety
    /// `ptr` must be a valid COM `IUnknown *` (or null). DirectComposition
    /// only accepts composition swap chains and a few other content types.
    pub unsafe fn from_raw_iunknown(ptr: *mut std::ffi::c_void) -> Self {
        Self {
            ptr,
            _not_send_sync: std::marker::PhantomData,
        }
    }

    pub fn as_raw(&self) -> *mut std::ffi::c_void {
        self.ptr
    }

    /// Relinquishes the reference without `Release`. Used when transferring
    /// the pointer into a Windows backend call that takes ownership.
    pub fn into_raw(self) -> *mut std::ffi::c_void {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }
}

#[cfg(target_os = "windows")]
impl Drop for DcompContent {
    fn drop(&mut self) {
        unsafe {
            use crate::windows::core::Interface;
            if !self.ptr.is_null() {
                drop(crate::windows::core::IUnknown::from_raw(self.ptr));
                self.ptr = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl Clone for DcompContent {
    fn clone(&self) -> Self {
        unsafe {
            use crate::windows::core::Interface;
            if self.ptr.is_null() {
                return Self {
                    ptr: std::ptr::null_mut(),
                    _not_send_sync: std::marker::PhantomData,
                };
            }
            let Some(unk) = crate::windows::core::IUnknown::from_raw_borrowed(&self.ptr) else {
                return Self {
                    ptr: std::ptr::null_mut(),
                    _not_send_sync: std::marker::PhantomData,
                };
            };
            Self {
                ptr: unk.clone().into_raw(),
                _not_send_sync: std::marker::PhantomData,
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl PartialEq for DcompContent {
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

#[cfg(target_os = "windows")]
impl std::fmt::Debug for DcompContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DcompContent")
            .field("ptr", &self.ptr)
            .finish()
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
    /// Create-time Windows DirectComposition path (`CreateSwapChainForComposition`).
    /// Ignored after the HWND exists; other platforms ignore it.
    #[live]
    pub direct_composition: bool,
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
            cx.windows[window_id].create_title =
                crate::remote::tag_window_title(self.title.clone());
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
        if !cx.windows[window_id].is_created {
            cx.windows[window_id].direct_composition = self.direct_composition;
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

    /// Whether this window currently presents through DirectComposition.
    ///
    /// False before the HWND exists, after it is closed, and on every
    /// non-Windows platform. The create-time request is `CxWindow::direct_composition`.
    pub fn is_direct_composition(&self, cx: &Cx) -> bool {
        let window = &cx.windows[self.window_id()];
        window.is_created && window.composition_active
    }

    pub fn set_backdrop_intensity(&mut self, cx: &mut Cx, backdrop_intensity: f32) {
        let mut visuals = cx.windows[self.window_id()].window_visuals();
        visuals.backdrop_intensity = backdrop_intensity;
        self.set_window_visuals(cx, visuals);
    }

    pub fn resize(&self, cx: &mut Cx, size: Vec2d) {
        cx.push_unique_platform_op(CxOsOp::ResizeWindow(self.window_id(), size));
    }

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

/// DirectComposition child visuals. Windows-only: the matching `CxOsOp` variants
/// are `cfg(target_os = "windows")`, so this whole impl is gated the same way.
#[cfg(target_os = "windows")]
impl WindowHandle {
    fn dcomp_should_queue(&self, cx: &Cx) -> bool {
        let window = &cx.windows[self.window_id()];
        !window.is_created || window.composition_active
    }

    /// Allocates a child visual behind or in front of the Makepad UI visual.
    ///
    /// The id is unique even when the op is not queued (HWND swap-chain host).
    pub fn dcomp_create_child(&mut self, cx: &mut Cx, z: DcompChildZ) -> DcompChildId {
        let window_id = self.window_id();
        let should_queue = self.dcomp_should_queue(cx);
        let window = &mut cx.windows[window_id];
        window.dcomp_child_seq = window.dcomp_child_seq.saturating_add(1);
        let child_id = DcompChildId(window.dcomp_child_seq);
        let z = z.sanitized();
        if should_queue {
            cx.platform_ops.push_back(CxOsOp::DcompCreateChild {
                window_id,
                child_id,
                z,
            });
        }
        child_id
    }

    pub fn dcomp_set_child_content(
        &mut self,
        cx: &mut Cx,
        child_id: DcompChildId,
        content: Option<DcompContent>,
    ) {
        if !self.dcomp_should_queue(cx) {
            return;
        }
        let window_id = self.window_id();
        cx.platform_ops.retain(|op| {
            !matches!(
                op,
                CxOsOp::DcompSetChildContent {
                    window_id: id,
                    child_id: cid,
                    ..
                } if *id == window_id && *cid == child_id
            )
        });
        cx.platform_ops.push_back(CxOsOp::DcompSetChildContent {
            window_id,
            child_id,
            content,
        });
    }

    /// Fills a child visual with one opaque colour. A child at
    /// [`DcompChildZ::BACKDROP`] that needs no geometry of its own is the usual
    /// caller: see that constant for why.
    ///
    /// Allocates a 1×1 composition swap chain per call, so call it once per
    /// child (or once per colour change) rather than from a per-frame sync.
    pub fn dcomp_set_child_solid(&mut self, cx: &mut Cx, child_id: DcompChildId, color: Vec4) {
        if !self.dcomp_should_queue(cx) {
            return;
        }
        let window_id = self.window_id();
        cx.platform_ops.retain(|op| {
            !matches!(
                op,
                CxOsOp::DcompSetChildSolid {
                    window_id: id,
                    child_id: cid,
                    ..
                } if *id == window_id && *cid == child_id
            )
        });
        cx.platform_ops.push_back(CxOsOp::DcompSetChildSolid {
            window_id,
            child_id,
            color,
        });
    }

    pub fn dcomp_set_child_geom(&mut self, cx: &mut Cx, child_id: DcompChildId, geom: DcompChildGeom) {
        if !self.dcomp_should_queue(cx) {
            return;
        }
        let window_id = self.window_id();
        cx.platform_ops.retain(|op| {
            !matches!(
                op,
                CxOsOp::DcompSetChildGeom {
                    window_id: id,
                    child_id: cid,
                    ..
                } if *id == window_id && *cid == child_id
            )
        });
        cx.platform_ops.push_back(CxOsOp::DcompSetChildGeom {
            window_id,
            child_id,
            geom,
        });
    }

    /// Moves an existing child to a different z. See [`DcompChildZ`].
    ///
    /// The host restacks every child against the new value, so the result does
    /// not depend on the order the children were created in.
    pub fn dcomp_set_child_z(&mut self, cx: &mut Cx, child_id: DcompChildId, z: DcompChildZ) {
        if !self.dcomp_should_queue(cx) {
            return;
        }
        let window_id = self.window_id();
        let z = z.sanitized();
        cx.platform_ops.retain(|op| {
            !matches!(
                op,
                CxOsOp::DcompSetChildZ {
                    window_id: id,
                    child_id: cid,
                    ..
                } if *id == window_id && *cid == child_id
            )
        });
        cx.platform_ops.push_back(CxOsOp::DcompSetChildZ {
            window_id,
            child_id,
            z,
        });
    }

    pub fn dcomp_remove_child(&mut self, cx: &mut Cx, child_id: DcompChildId) {
        if !self.dcomp_should_queue(cx) {
            return;
        }
        let window_id = self.window_id();
        let mut dropped_create = false;
        cx.platform_ops.retain(|op| {
            let same_child = match op {
                CxOsOp::DcompCreateChild {
                    window_id: id,
                    child_id: cid,
                    ..
                }
                | CxOsOp::DcompSetChildContent {
                    window_id: id,
                    child_id: cid,
                    ..
                }
                | CxOsOp::DcompSetChildSolid {
                    window_id: id,
                    child_id: cid,
                    ..
                }
                | CxOsOp::DcompSetChildGeom {
                    window_id: id,
                    child_id: cid,
                    ..
                }
                | CxOsOp::DcompSetChildZ {
                    window_id: id,
                    child_id: cid,
                    ..
                }
                | CxOsOp::DcompRemoveChild {
                    window_id: id,
                    child_id: cid,
                } => *id == window_id && *cid == child_id,
                _ => false,
            };
            if same_child {
                dropped_create |= matches!(op, CxOsOp::DcompCreateChild { .. });
                false
            } else {
                true
            }
        });
        if !dropped_create {
            cx.platform_ops.push_back(CxOsOp::DcompRemoveChild {
                window_id,
                child_id,
            });
        }
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
    pub direct_composition: bool,
    /// Set when the native window actually bound a composition tree.
    pub composition_active: bool,
    #[cfg(target_os = "windows")]
    pub(crate) dcomp_child_seq: u64,
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
            direct_composition: false,
            composition_active: false,
            #[cfg(target_os = "windows")]
            dcomp_child_seq: 0,
            backdrop: WindowBackdrop::None,
            backdrop_intensity: 1.0,
            macos: MacosWindowConfig::default(),
        }
    }
}

impl CxWindow {
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

    fn test_cx() -> Cx {
        Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}))
    }

    #[test]
    fn window_visuals_defaults() {
        let mut cx = test_cx();
        let window = WindowHandle::new(&mut cx);
        let cx_window = &cx.windows[window.window_id()];
        assert!(!cx_window.transparent);
        assert_eq!(cx_window.backdrop, WindowBackdrop::None);
        assert_eq!(cx_window.backdrop_intensity, 1.0);
        assert!(!cx_window.composition_active);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_create_child_allocates_ids_and_queues_ops() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        cx.platform_ops.clear();

        let a = window.dcomp_create_child(&mut cx, DcompChildZ::BEHIND);
        let b = window.dcomp_create_child(&mut cx, DcompChildZ::FRONT);
        assert_ne!(a, b);

        window.dcomp_set_child_geom(
            &mut cx,
            a,
            DcompChildGeom {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
                visible: true,
                ..Default::default()
            },
        );
        window.dcomp_set_child_geom(
            &mut cx,
            a,
            DcompChildGeom {
                x: 10.0,
                y: 20.0,
                width: 30.0,
                height: 40.0,
                visible: true,
                ..Default::default()
            },
        );

        let geom_ops = cx
            .platform_ops
            .iter()
            .filter(|op| matches!(op, CxOsOp::DcompSetChildGeom { child_id, .. } if *child_id == a))
            .count();
        assert_eq!(geom_ops, 1);
        assert!(matches!(
            cx.platform_ops.back(),
            Some(CxOsOp::DcompSetChildGeom {
                geom: DcompChildGeom {
                    x: 10.0,
                    width: 30.0,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_child_geom_sanitizes_non_finite_and_negative_extent() {
        let geom = DcompChildGeom {
            x: f32::NAN,
            y: f32::NEG_INFINITY,
            width: -8.0,
            height: f32::INFINITY,
            visible: true,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(geom.x, 0.0);
        assert_eq!(geom.y, 0.0);
        assert_eq!(geom.width, 0.0);
        assert_eq!(geom.height, 0.0);
        assert!(!geom.is_shown());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_child_z_is_relative_to_ui_at_zero() {
        assert_eq!(DcompChildZ::UI.0, 0);
        assert_eq!(DcompChildZ(0).sanitized(), DcompChildZ::BEHIND);
        assert!(DcompChildZ::BACKDROP < DcompChildZ::BEHIND);
        assert!(DcompChildZ::BEHIND < DcompChildZ::FRONT);
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        cx.platform_ops.clear();
        let _ = window.dcomp_create_child(&mut cx, DcompChildZ(0));
        assert!(matches!(
            cx.platform_ops.back(),
            Some(CxOsOp::DcompCreateChild {
                z: DcompChildZ(-1),
                ..
            })
        ));
    }

    #[test]
    fn is_direct_composition_is_false_until_the_hwnd_is_bound() {
        let mut cx = test_cx();
        let window = WindowHandle::new(&mut cx);
        cx.windows[window.window_id()].direct_composition = true;
        assert!(!window.is_direct_composition(&cx));
        cx.windows[window.window_id()].is_created = true;
        cx.windows[window.window_id()].composition_active = true;
        assert!(window.is_direct_composition(&cx));
        cx.windows[window.window_id()].is_created = false;
        cx.windows[window.window_id()].composition_active = false;
        assert!(!window.is_direct_composition(&cx));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_create_child_does_not_queue_on_an_hwnd_swapchain_host() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        cx.windows[window.window_id()].is_created = true;
        cx.windows[window.window_id()].composition_active = false;
        cx.platform_ops.clear();
        let _ = window.dcomp_create_child(&mut cx, DcompChildZ::BEHIND);
        assert!(cx.platform_ops.is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_remove_child_cancels_pending_create() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        cx.platform_ops.clear();
        let id = window.dcomp_create_child(&mut cx, DcompChildZ::BEHIND);
        window.dcomp_set_child_geom(
            &mut cx,
            id,
            DcompChildGeom {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
                visible: true,
                ..Default::default()
            },
        );
        window.dcomp_set_child_solid(&mut cx, id, vec4(0.0, 0.0, 0.0, 1.0));
        window.dcomp_set_child_z(&mut cx, id, DcompChildZ::FRONT);
        window.dcomp_remove_child(&mut cx, id);
        assert!(cx.platform_ops.is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_set_child_z_keeps_only_the_last_value_per_child() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        let a = window.dcomp_create_child(&mut cx, DcompChildZ::BEHIND);
        let b = window.dcomp_create_child(&mut cx, DcompChildZ::BEHIND);
        cx.platform_ops.clear();
        window.dcomp_set_child_z(&mut cx, a, DcompChildZ::FRONT);
        window.dcomp_set_child_z(&mut cx, b, DcompChildZ::BACKDROP);
        // Reserved z snaps to BEHIND here, not in the backend, so the op that
        // reaches the host is already valid.
        window.dcomp_set_child_z(&mut cx, a, DcompChildZ::UI);
        let zs: Vec<_> = cx
            .platform_ops
            .iter()
            .filter_map(|op| match op {
                CxOsOp::DcompSetChildZ { child_id, z, .. } => Some((*child_id, *z)),
                _ => None,
            })
            .collect();
        assert_eq!(
            zs,
            vec![(b, DcompChildZ::BACKDROP), (a, DcompChildZ::BEHIND)]
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_set_child_solid_dedups_per_child() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        let a = window.dcomp_create_child(&mut cx, DcompChildZ::BACKDROP);
        let b = window.dcomp_create_child(&mut cx, DcompChildZ::BACKDROP);
        cx.platform_ops.clear();
        window.dcomp_set_child_solid(&mut cx, a, vec4(1.0, 0.0, 0.0, 1.0));
        window.dcomp_set_child_solid(&mut cx, b, vec4(0.0, 1.0, 0.0, 1.0));
        window.dcomp_set_child_solid(&mut cx, a, vec4(0.0, 0.0, 1.0, 1.0));
        let solids: Vec<_> = cx
            .platform_ops
            .iter()
            .filter_map(|op| match op {
                CxOsOp::DcompSetChildSolid {
                    child_id, color, ..
                } => Some((*child_id, *color)),
                _ => None,
            })
            .collect();
        assert_eq!(
            solids,
            vec![(b, vec4(0.0, 1.0, 0.0, 1.0)), (a, vec4(0.0, 0.0, 1.0, 1.0))]
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_content_and_geom_queue_before_create() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        cx.platform_ops.clear();
        let id = DcompChildId(7);
        window.dcomp_set_child_content(&mut cx, id, None);
        window.dcomp_set_child_geom(
            &mut cx,
            id,
            DcompChildGeom {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
                visible: true,
                ..Default::default()
            },
        );
        assert!(matches!(
            cx.platform_ops.front(),
            Some(CxOsOp::DcompSetChildContent { child_id, .. }) if *child_id == id
        ));
        assert!(matches!(
            cx.platform_ops.back(),
            Some(CxOsOp::DcompSetChildGeom { child_id, .. }) if *child_id == id
        ));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dcomp_child_geom_approx_eq_tolerates_subpixel_jitter() {
        let a = DcompChildGeom {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            visible: true,
            ..Default::default()
        };
        let mut b = a;
        b.x += 0.25;
        b.width += 0.4;
        assert!(a.approx_eq(b));
        b.x += 1.0;
        assert!(!a.approx_eq(b));
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
        let mut std = ();
        let mut vm = ScriptVm {
            host: &mut host,
            std: &mut std,
            bx: Box::new(ScriptVmBase::new()),
        };

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
        config.on_after_apply(&mut vm, &Apply::New, &mut Scope::empty(), obj.into());

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
        let mut std = ();
        let mut vm = ScriptVm {
            host: &mut host,
            std: &mut std,
            bx: Box::new(ScriptVmBase::new()),
        };

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
            direct_composition: false,
            backdrop: WindowBackdrop::Blur,
            backdrop_intensity: 0.5,
            macos: MacosWindowConfig::floating_panel(),
            caption_bar_height_override: None,
        };

        script_window.on_after_apply(&mut vm, &Apply::New, &mut Scope::empty(), NIL);

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
