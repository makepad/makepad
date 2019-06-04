use crate::cx::*;


#[derive(Clone)]
pub struct Window {
    pub window_id: Option<usize>,
    pub create_inner_size: Vec2,
    pub create_pos: Option<Vec2>,
    pub create_title: String,
}

impl Style for Window {
    fn style(_cx: &mut Cx) -> Self {
        Self {
            window_id: None,
            create_inner_size: Vec2{x:800., y:600.},
            create_pos: None,
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
                    position:self.create_pos,
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
        cx.windows[window_id].root_draw_list_id = None;
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
    
    pub fn end_window(&mut self, cx: &mut Cx) -> Area {
        cx.window_stack.pop();
        Area::Empty
    }
}