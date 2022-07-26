pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },

    crate::{
        makepad_live_compiler::*,
        makepad_live_id::*,
        makepad_math::*,
        event::{
            WindowGeom
        },
        area::Area,
        pass::{Pass,CxPassParent},
        cx::Cx,
        cx_api::CxPlatformOp,
        live_traits::*,
    }
};

pub struct Window {
    pub(crate) window_id: usize,
    pub(crate) windows_free: Rc<RefCell<Vec<usize>>>,
}

impl Drop for Window{
    fn drop(&mut self){
        self.windows_free.borrow_mut().push(self.window_id)
    }
}
impl LiveHook for Window{}
impl LiveNew for Window {
    fn new(cx: &mut Cx)->Self{
        let windows_free = cx.windows_free.clone();
        let window_id =  if let Some(view_id) = windows_free.borrow_mut().pop(  ){
            view_id 
        }
        else{
            let window_id = cx.windows.len();
            cx.windows.push(CxWindow {
                is_created:false,
                create_title:"Makepad".to_string(),
                create_inner_size: cx.get_default_window_size(),
                create_position: None,
                ..Default::default()
            });
            cx.platform_ops.push(CxPlatformOp::CreateWindow(window_id));
            window_id
        };
        
        Self {
            windows_free,
            window_id,
        }
    }
    
    fn live_type_info(_cx:&mut Cx) -> LiveTypeInfo{
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            fields: Vec::new(),
            live_ignore: true,
            type_name: LiveId::from_str("Window").unwrap()
        }
    }
}
impl LiveApply for Window {
    //fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>()}
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, id!(View));
            return nodes.skip_node(start_index);
        }
        
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                id!(inner_size) => {
                    let v = LiveNew::new_apply_mut_index(cx, from, &mut index, nodes);
                    cx.windows[self.window_id].create_inner_size = v;
                },
                id!(title) => {
                    let v = LiveNew::new_apply_mut_index(cx, from, &mut index, nodes);
                    cx.windows[self.window_id].create_title = v;
                }
                id!(position) => {
                    let v = LiveNew::new_apply_mut_index(cx, from, &mut index, nodes);
                    cx.windows[self.window_id].create_position = Some(v);
                }
                _=> {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    index = nodes.skip_node(index);
                }
            }
        }
        return index;
    }
}


impl Window {
    pub fn window_id(&self)->usize{self.window_id}
    
    pub fn set_pass(&self, cx:&mut Cx, pass:&Pass){
        cx.windows[self.window_id].main_pass_id = Some(pass.pass_id);
        cx.passes[pass.pass_id].parent = CxPassParent::Window(self.window_id);
    }

    pub fn get_inner_size(&mut self, cx: &mut Cx) -> Vec2 {
        cx.windows[self.window_id].get_inner_size()
    }
    
    pub fn get_position(&mut self, cx: &mut Cx) -> Vec2 {
        cx.windows[self.window_id].get_position()
    }

    pub fn minimize(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxPlatformOp::MinimizeWindow(self.window_id));
    }
    
    pub fn maximize(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxPlatformOp::MaximizeWindow(self.window_id));
    }
    
    pub fn fullscreen(&mut self, cx:&mut Cx){
        cx.push_unique_platform_op(CxPlatformOp::FullscreenWindow(self.window_id));
    }
    
    pub fn normal(&mut self, cx:&mut Cx){
        cx.push_unique_platform_op(CxPlatformOp::NormalizeWindow(self.window_id));
    }
        
    pub fn can_fullscreen(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id].window_geom.can_fullscreen
    }
    
    pub fn xr_can_present(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id].window_geom.xr_can_present
    }
    
    pub fn is_fullscreen(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id].window_geom.is_fullscreen
    }
    
    pub fn xr_is_presenting(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id].window_geom.xr_is_presenting
    }
    
    pub fn xr_start_presenting(&mut self, cx: &mut Cx){
        cx.push_unique_platform_op(CxPlatformOp::XrStartPresenting(self.window_id));
    }
    
    pub fn xr_stop_presenting(&mut self, cx: &mut Cx){
        cx.push_unique_platform_op(CxPlatformOp::XrStopPresenting(self.window_id));
    }
    
    pub fn is_topmost(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id].window_geom.is_topmost
    }
    
    pub fn set_topmost(&mut self, cx: &mut Cx, set_topmost:bool) {
        cx.push_unique_platform_op(CxPlatformOp::SetTopmost(self.window_id, set_topmost));
    }
    
    pub fn restore(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxPlatformOp::RestoreWindow(self.window_id));
    }
    
    pub fn close(&mut self, cx: &mut Cx) {
        cx.push_unique_platform_op(CxPlatformOp::CloseWindow(self.window_id));
    }
}

#[derive(Clone, Default)]
pub struct CxWindow {
    pub create_title: String,
    pub create_position: Option<Vec2>,
    pub create_inner_size: Vec2,
    pub is_created: bool,
    pub window_geom: WindowGeom,
    pub main_pass_id: Option<usize>,
}

impl CxWindow {
    
    pub fn get_inner_size(&mut self) -> Vec2 {
        if !self.is_created{
            self.create_inner_size
        }
        else{
            self.window_geom.inner_size
        }
    }
    
    pub fn get_position(&mut self) -> Vec2 {
        if !self.is_created{
            self.create_position.unwrap()
        }
        else{
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