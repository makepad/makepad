use {
    crate::{
        //makepad_live_id::*,
        makepad_script::*,
        script::vm::*,
        makepad_math::*,
        makepad_error_log::*,
        id_pool::*,
        event::{
            WindowGeom
        },
        draw_pass::{DrawPass, DrawPassId, CxDrawPassParent},
        cx::Cx,
        cx_api::CxOsOp,
    }
};

pub struct WindowHandle(PoolId);

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct WindowId(pub usize, pub u64);

impl WindowId{
    pub fn id(&self)->usize{self.0}
}

impl WindowHandle {
    pub fn window_id(&self) -> WindowId {WindowId(self.0.id, self.0.generation)}
}

#[derive(Default)]
pub struct CxWindowPool(IdPool<CxWindow>);
impl CxWindowPool {
    fn alloc(&mut self) -> WindowHandle {
        WindowHandle(self.0.alloc())
    }
    
    pub fn window_id_contains(&self, pos:Vec2d)->(WindowId, Vec2d){
        for (index,item) in self.0.pool.iter().enumerate(){
            let window = &item.item;
            if pos.x>= window.window_geom.position.x &&
                pos.y>= window.window_geom.position.y && 
                pos.x<= window.window_geom.position.x+window.window_geom.inner_size.x  &&
                pos.y<= window.window_geom.position.y+window.window_geom.inner_size.y{
                return (WindowId(index, item.generation), window.window_geom.position)
            }
        }
        return (WindowId(0, self.0.pool[0].generation), self.0.pool[0].item.window_geom.position)
    }
    
    
    pub fn relative_to_window_id(&self, pos:Vec2d)->(WindowId, Vec2d){
        for (index,item) in self.0.pool.iter().enumerate(){
            let window = &item.item;
            if pos.x>= window.window_geom.position.x &&
            pos.y>= window.window_geom.position.y && 
            pos.x<= window.window_geom.position.x+window.window_geom.inner_size.x  &&
            pos.y<= window.window_geom.position.x+window.window_geom.inner_size.y{
                return (WindowId(index, item.generation), window.window_geom.position)
            }
        }
        return (WindowId(0, self.0.pool[0].generation), self.0.pool[0].item.window_geom.position)
    }
    
    pub fn is_valid(&self, v: WindowId)->bool{
        if v.0 < self.0.pool.len(){
            if self.0.pool[v.0].generation == v.1{
                return true
            }
        }
        false
    }
    
    pub fn id_zero()->WindowId{
        WindowId(0, 0)
    }
    
    pub fn from_usize(v:usize)->WindowId{
        WindowId(v, 0)
    }
}

impl std::ops::Index<WindowId> for CxWindowPool {
    type Output = CxWindow;
    fn index(&self, index: WindowId) -> &Self::Output {
        let d = &self.0.pool[index.0];
        if d.generation != index.1{
            error!("Window id generation wrong {} {} {}", index.0, d.generation, index.1)
        }
        &d.item
    }
}

impl std::ops::IndexMut<WindowId> for CxWindowPool {
    fn index_mut(&mut self, index: WindowId) -> &mut Self::Output {
        let d = &mut self.0.pool[index.0];
        if d.generation != index.1{
            error!("Window id generation wrong {} {} {}", index.0, d.generation, index.1)
        }
        &mut d.item
    }
}

impl ScriptHook for WindowHandle {}
impl ScriptNew for WindowHandle {
    fn script_new(vm:&mut ScriptVm)->Self{
        let cx = vm.cx_mut();
        let window = cx.windows.alloc();
        let cxwindow = &mut cx.windows[window.window_id()];
        cxwindow.is_created = false;
        cxwindow.create_title = "Makepad".to_string();
        cxwindow.create_inner_size = None;
        cxwindow.create_position = None;
        cx.platform_ops.push(CxOsOp::CreateWindow(window.window_id()));
        window
    }
}

impl ScriptApply for WindowHandle {
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&Apply, _scope:&mut Scope, value:ScriptValue) {
        if let Some(v) = ScriptNew::script_from_dirty(vm, value, id!(inner_size)){
            vm.host.cx_mut().windows[self.window_id()].create_inner_size = Some(v);
        }
        if let Some(v) = ScriptNew::script_from_dirty(vm, value, id!(title)){
            vm.host.cx_mut().windows[self.window_id()].create_title = v;
        }
        if let Some(v) = ScriptNew::script_from_dirty(vm, value, id!(kind_id)){
            vm.host.cx_mut().windows[self.window_id()].kind_id = v;
        }
        if let Some(v) = ScriptNew::script_from_dirty(vm, value, id!(position)){
            vm.host.cx_mut().windows[self.window_id()].create_position = Some(v);
        }
        if let Some(v) = ScriptNew::script_from_dirty(vm, value, id!(dpi_override)){
            vm.host.cx_mut().windows[self.window_id()].dpi_override = Some(v);
        }
        if let Some(v) = ScriptNew::script_from_dirty(vm, value, id!(topmost)){
            self.set_topmost(vm.host.cx_mut(), v);
        }
    }
}

impl WindowHandle {
    pub fn set_pass(&self, cx: &mut Cx, pass: &DrawPass) {
        cx.windows[self.window_id()].main_pass_id = Some(pass.draw_pass_id());
        cx.passes[pass.draw_pass_id()].parent = CxDrawPassParent::Window(self.window_id());
    }
    pub fn configure_window(&mut self, cx: &mut Cx, inner_size: Vec2d, position: Vec2d, is_fullscreen: bool, title: String) {
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
    
    pub fn set_kind_id(&mut self, cx: &mut Cx,kind_id:usize) {
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

#[derive(Clone, Default)]
pub struct CxWindow {
    pub create_title: String,
    pub create_position: Option<Vec2d>,
    pub create_inner_size: Option<Vec2d>,
    pub kind_id: usize,
    pub dpi_override: Option<f64>,
    pub os_dpi_factor: Option<f64>,
    pub is_created: bool,
    pub window_geom: WindowGeom,
    pub main_pass_id: Option<DrawPassId>,
    pub is_fullscreen: bool,
}

impl CxWindow {
    pub fn remap_dpi_override(&self, pos:Vec2d)->Vec2d{
        if let Some(dpi_override) = self.dpi_override{
            if let Some(os_dpi_factor) = self.os_dpi_factor{
                return pos * ( os_dpi_factor / dpi_override)
            }
        }
        return pos
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