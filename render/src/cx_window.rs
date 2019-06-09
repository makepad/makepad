use crate::cx::*;

#[derive(Default, Clone)]
pub struct Window {
    pub window_id: Option<usize>,
    pub create_inner_size: Vec2,
    pub create_position: Option<Vec2>,
    pub create_title: String,
}

impl Style for Window {
    fn style(_cx: &mut Cx) -> Self {
        Self {
            window_id: None,
            create_inner_size: Vec2{x:800., y:600.},
            create_position: None,
            create_title: "Makepad".to_string()
        }
    }
}

impl Window {
    
    pub fn begin_window(&mut self, cx: &mut Cx) {
        // if we are not at ground level for viewports,
        if self.window_id.is_none() {
            let new_window = CxWindow {
                window_state:CxWindowState::Create{
                    title:self.create_title.clone(),
                    inner_size:self.create_inner_size,
                    position:self.create_position,
                },
                ..Default::default()
             };
            let window_id;
            if cx.windows_free.len() != 0 {
                window_id = cx.windows_free.pop().unwrap();
                cx.windows[window_id] = new_window
            }
            else {
                window_id = cx.windows.len();
                cx.windows.push(new_window);
            }
            self.window_id = Some(window_id);
        }
        let window_id = self.window_id.unwrap();
        cx.windows[window_id].main_pass_id = None;
        cx.window_stack.push(window_id);
        
    }
    
    pub fn get_inner_size(&mut self, cx: &mut Cx)->Vec2{
        if let Some(window_id) = self.window_id{
            return cx.windows[window_id].get_inner_size()
        }
        return Vec2::zero();
    }

    pub fn get_position(&mut self, cx: &mut Cx)->Option<Vec2>{
        if let Some(window_id) = self.window_id{
            return cx.windows[window_id].get_position()
        }
        return None
    }
    
    pub fn handle_window(&mut self, _cx: &mut Cx, _event: &mut Event) -> bool {
        false
    }
    
    pub fn redraw_window_area(&mut self, cx: &mut Cx){
        if let Some(window_id) = self.window_id{
            if let Some(pass_id) = cx.windows[window_id].main_pass_id{
                cx.redraw_pass_and_sub_passes(pass_id);
            }
        }
    }
    
    pub fn end_window(&mut self, cx: &mut Cx) -> Area {
        cx.window_stack.pop();
        Area::Empty
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct WindowGeom {
    pub dpi_factor: f32,
    pub is_fullscreen: bool,
    pub position: Vec2,
    pub inner_size: Vec2,
    pub outer_size: Vec2,
}

#[derive(Clone)]
pub enum CxWindowState{
    Create{title:String, inner_size:Vec2, position:Option<Vec2>},
    Created,
    Destroy,
    Destroyed
}

impl Default for CxWindowState{
    fn default()->Self{CxWindowState::Destroyed}
}

#[derive(Clone, Default)]
pub struct CxWindow {
    pub window_state:CxWindowState,
    pub window_geom: WindowGeom,
    pub main_pass_id: Option<usize>,
}

impl CxWindow{
    pub fn get_inner_size(&mut self)->Vec2{
        match &self.window_state{
            CxWindowState::Create{inner_size,..}=>*inner_size,
            CxWindowState::Created=>self.window_geom.inner_size,
            _=>Vec2::zero()
        }
    }

    pub fn get_position(&mut self)->Option<Vec2>{
        match &self.window_state{
            CxWindowState::Create{position,..}=>*position,
            CxWindowState::Created=>Some(self.window_geom.position),
            _=>None
        }
    }

    pub fn get_dpi_factor(&mut self)->Option<f32>{
        match &self.window_state{
            CxWindowState::Created=>Some(self.window_geom.dpi_factor),
            _=>None
        }
    }
}