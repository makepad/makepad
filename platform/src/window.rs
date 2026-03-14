use crate::{
    cx::Cx,
    cx_api::CxOsOp,
    draw_pass::{CxDrawPassParent, DrawPass, DrawPassId},
    event::WindowGeom,
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

impl WindowHandle {
    pub fn new(cx: &mut Cx) -> Self {
        let window = cx.windows.alloc();
        let cxwindow = &mut cx.windows[window.window_id()];
        cxwindow.is_created = false;
        cxwindow.create_title = "Makepad".to_string();
        cxwindow.create_inner_size = None;
        cxwindow.create_position = None;
        cxwindow.create_app_id = "Makepad".to_string();
        cxwindow.is_popup = false;
        cxwindow.popup_parent = None;
        cxwindow.popup_position = None;
        cxwindow.popup_size = None;
        cxwindow.popup_grab_keyboard = true;
        cx.platform_ops
            .push(CxOsOp::CreateWindow(window.window_id()));
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
            cxwindow.create_app_id = "Makepad".to_string();
            cxwindow.is_popup = true;
            cxwindow.popup_parent = Some(parent);
            cxwindow.popup_position = Some(position);
            cxwindow.popup_size = Some(size);
            cxwindow.popup_grab_keyboard = true;
            cxwindow.popup_grab_keyboard
        };
        cx.platform_ops.push(CxOsOp::CreatePopupWindow {
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
            cx.windows[window_id].create_title = self.title.clone();
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
        window.create_title = title;
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
    pub fn window_visuals(&self) -> WindowVisuals {
        WindowVisuals {
            transparent: self.transparent,
            backdrop: self.backdrop,
            backdrop_intensity: self.backdrop_intensity,
        }
        .normalized()
    }

    pub fn remap_dpi_override(&self, pos: Vec2d) -> Vec2d {
        if let Some(dpi_override) = self.dpi_override {
            if let Some(os_dpi_factor) = self.os_dpi_factor {
                return pos * (os_dpi_factor / dpi_override);
            }
        }
        return pos;
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
            inner_size: Some(dvec2(320.0, 80.0)),
            position: Some(dvec2(40.0, 50.0)),
            kind_id: 7,
            dpi_override: Some(2.0),
            topmost: false,
            transparent: true,
            backdrop: WindowBackdrop::Blur,
            backdrop_intensity: 0.5,
            macos: MacosWindowConfig::floating_panel(),
        };

        script_window.on_after_apply(&mut vm, &Apply::New, &mut Scope::empty(), NIL);

        let cx = vm.cx_mut();
        let cx_window = &cx.windows[window_id];
        assert_eq!(cx_window.create_title, "Floating Panel");
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
    fn set_topmost_queues_platform_op() {
        let mut cx = test_cx();
        let mut window = WindowHandle::new(&mut cx);
        cx.platform_ops.clear();

        window.set_topmost(&mut cx, true);

        assert_eq!(cx.platform_ops.len(), 1);
        assert!(matches!(cx.platform_ops[0], CxOsOp::SetTopmost(_, true)));
    }
}
