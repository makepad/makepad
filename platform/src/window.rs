use {
    crate::{
        makepad_live_compiler::*,
        makepad_live_id::*,
        makepad_math::*,
        id_pool::*,
        event::{
            WindowGeom
        },
        pass::{Pass, PassId, CxPassParent},
        cx::Cx,
        cx_api::CxOsOp,
        live_traits::*,
    }
};

pub struct WindowHandle(PoolId);

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct WindowId(usize, u64);

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
    
    pub fn window_id_contains(&self, pos:DVec2)->(WindowId, DVec2){
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
    
    
    pub fn relative_to_window_id(&self, pos:DVec2)->(WindowId, DVec2){
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

impl LiveHook for WindowHandle {}
impl LiveNew for WindowHandle {
    fn new(cx: &mut Cx) -> Self {
        let window = cx.windows.alloc();
        let cxwindow = &mut cx.windows[window.window_id()];
        cxwindow.is_created = false;
        cxwindow.create_title = "Makepad".to_string();
        cxwindow.create_inner_size = None;
        cxwindow.create_position = None;
        cx.platform_ops.push(CxOsOp::CreateWindow(window.window_id()));
        window
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            fields: Vec::new(),
            live_ignore: true,
            type_name: id_lut!(Window)
        }
    }
}
impl LiveApply for WindowHandle {
    //fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>()}
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, live_id!(View));
            return nodes.skip_node(start_index);
        }
        
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                live_id!(inner_size) => {
                    let v:Vec2 = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes);
                    cx.windows[self.window_id()].create_inner_size = Some(v.into());
                },
                live_id!(title) => {
                    let v = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes);
                    cx.windows[self.window_id()].create_title = v;
                }
                live_id!(kind_id) => {
                    let v = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes);
                    cx.windows[self.window_id()].kind_id = v;
                }
                live_id!(position) => {
                    let v:Vec2 = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes);
                    cx.windows[self.window_id()].create_position = Some(v.into());
                }
                live_id!(dpi_override) => {
                    let v:f64 = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes);
                    //log!("DPI OVERRIDE {}", v);
                    cx.windows[self.window_id()].dpi_override = Some(v);
                }
                live_id!(topmost) => {
                    let v:bool = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes);
                    self.set_topmost(cx, v);
                }
                _ => {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    index = nodes.skip_node(index);
                }
            }
        }
        return index;
    }
}


impl WindowHandle {
    pub fn set_pass(&self, cx: &mut Cx, pass: &Pass) {
        cx.windows[self.window_id()].main_pass_id = Some(pass.pass_id());
        cx.passes[pass.pass_id()].parent = CxPassParent::Window(self.window_id());
    }
    
    pub fn get_inner_size(&self, cx: &Cx) -> DVec2 {
        cx.windows[self.window_id()].get_inner_size()
    }
    
    pub fn get_position(&self, cx: &Cx) -> DVec2 {
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
    
    pub fn is_fullscreen(&mut self, cx: &mut Cx) -> bool {
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
    pub create_position: Option<DVec2>,
    pub create_inner_size: Option<DVec2>,
    pub kind_id: usize,
    pub dpi_override: Option<f64>,
    pub is_created: bool,
    pub window_geom: WindowGeom,
    pub main_pass_id: Option<PassId>,
}

impl CxWindow {
    
    pub fn get_inner_size(&self) -> DVec2 {
        if !self.is_created {
            Default::default()
            //panic!();
        }
        else {
            self.window_geom.inner_size
        }
    }
    
    pub fn get_position(&self) -> DVec2 {
        if !self.is_created {
            panic!();
        }
        else {
            self.window_geom.position
        }
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