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
        if self.topmost {
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
        }
    }
}

impl CxWindow {
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
