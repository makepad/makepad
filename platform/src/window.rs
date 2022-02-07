pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },

    crate::{
        makepad_live_compiler::*,
        makepad_math::*,
        event::{
            WindowGeom
        },
        area::Area,
        pass::{Pass,CxPassParent},
        cx::Cx,
        live_traits::*,
    }
};

pub struct Window {
    pub window_id: usize,
    pub windows_free: Rc<RefCell<Vec<usize>>>,
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
                window_state: CxWindowState::Create {
                    title: "Fill in here".to_string(),
                    inner_size: cx.get_default_window_size(),
                    position: None
                },
                ..Default::default()
            });
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
           // kind: LiveTypeKind::Object,
            type_name: LiveId::from_str("Window").unwrap()
        }
    }
}
impl LiveApply for Window {
    //fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>()}
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
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
                id!(position) => cx.windows[self.window_id].window_set_position = Some(LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes)),
                id!(inner_size) => {
                    let v = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes);
                    if let CxWindowState::Create{inner_size,..} = &mut cx.windows[self.window_id].window_state{
                        *inner_size = v;
                    }
                },
                id!(title) => {
                    let v = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes);
                    if let CxWindowState::Create{title,..} = &mut cx.windows[self.window_id].window_state{
                        *title = v;
                    }
                }
                id!(position) => {
                    let v = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes);
                    if let CxWindowState::Create{position,..} = &mut cx.windows[self.window_id].window_state{
                        *position = v;
                    }
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
    
    pub fn set_pass(&self, cx:&mut Cx, pass:&Pass){
        cx.windows[self.window_id].main_pass_id = Some(pass.pass_id);
        cx.passes[pass.pass_id].parent = CxPassParent::Window(self.window_id);
    }

    pub fn get_inner_size(&mut self, cx: &mut Cx) -> Vec2 {
        return cx.windows[self.window_id].get_inner_size()
    }
    
    pub fn get_position(&mut self, cx: &mut Cx) -> Option<Vec2> {
        return cx.windows[self.window_id].get_position()
    }
    
        
    pub fn set_position(&mut self, cx: &mut Cx, pos:Vec2) {
        return cx.windows[self.window_id].window_set_position = Some(pos);
    }

    pub fn minimize(&mut self, cx: &mut Cx) {
        cx.windows[self.window_id].window_command = CxWindowCmd::Minimize;
    }
    
    pub fn maximize(&mut self, cx: &mut Cx) {
        cx.windows[self.window_id].window_command = CxWindowCmd::Maximize;
    }
    
    pub fn fullscreen(&mut self, cx:&mut Cx){
        cx.windows[self.window_id].window_command = CxWindowCmd::FullScreen;
    }
    
    pub fn normal(&mut self, cx:&mut Cx){
        cx.windows[self.window_id].window_command = CxWindowCmd::NormalScreen;
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
        cx.windows[self.window_id].window_command = CxWindowCmd::XrStartPresenting;
    }
    
    pub fn xr_stop_presenting(&mut self, cx: &mut Cx){
        cx.windows[self.window_id].window_command = CxWindowCmd::XrStopPresenting;
    }
    
    pub fn is_topmost(&mut self, cx: &mut Cx) -> bool {
        cx.windows[self.window_id].window_geom.is_topmost
    }
    
    pub fn set_topmost(&mut self, cx: &mut Cx, topmost: bool) {
        cx.windows[self.window_id].window_topmost = Some(topmost);
    }
    
    pub fn restore(&mut self, cx: &mut Cx) {
        cx.windows[self.window_id].window_command = CxWindowCmd::Restore;
    }
    
    pub fn close(&mut self, cx: &mut Cx) {
        cx.windows[self.window_id].window_state = CxWindowState::Close;
    }
}

#[derive(Clone)]
pub enum CxWindowState {
    Create {title: String, inner_size: Vec2, position: Option<Vec2>},
    Created,
    Close,
    Closed
}

#[derive(Clone)]
pub enum CxWindowCmd {
    None,
    Restore,
    Maximize,
    Minimize,
    XrStartPresenting,
    XrStopPresenting,
    FullScreen,
    NormalScreen
}

impl Default for CxWindowCmd {
    fn default() -> Self {CxWindowCmd::None}
}

impl Default for CxWindowState {
    fn default() -> Self {CxWindowState::Closed}
}

#[derive(Clone, Default)]
pub struct CxWindow {
    pub window_state: CxWindowState,
    pub window_command: CxWindowCmd,
    pub window_set_position: Option<Vec2>,
    pub window_topmost: Option<bool>,
    pub window_geom: WindowGeom,
    pub main_pass_id: Option<usize>,
}

impl CxWindow {
    pub fn get_inner_size(&mut self) -> Vec2 {
        match &self.window_state {
            CxWindowState::Create {inner_size, ..} => *inner_size,
            CxWindowState::Created => self.window_geom.inner_size,
            _ => Vec2::default()
        }
    }
    
    pub fn get_position(&mut self) -> Option<Vec2> {
        match &self.window_state {
            CxWindowState::Create {position, ..} => *position,
            CxWindowState::Created => Some(self.window_geom.position),
            _ => None
        }
    }
    
    pub fn get_dpi_factor(&mut self) -> Option<f32> {
        match &self.window_state {
            CxWindowState::Created => Some(self.window_geom.dpi_factor),
            _ => None
        }
    }
}