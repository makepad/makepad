use crate::{
    makepad_draw_2d::*,
    scroll_bar::*
};

live_register!{
    ScrollView:{{ScrollView}}{
        show_scroll_x:true,
        show_scroll_y:true,
        view:{debug_id:scroll_inner_view}
    }
}

impl std::ops::Deref for ScrollView {
    type Target = View;
    fn deref(&self) -> &Self::Target {&self.view}
}

impl std::ops::DerefMut for ScrollView {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.view}
}

#[derive(Live, LiveHook)]
pub struct ScrollView{
    pub view:View,
    pub scroll: Vec2,
    pub show_scroll_x:bool,
    pub show_scroll_y:bool,
    pub scroll_bar_x:ScrollBar,
    pub scroll_bar_y:ScrollBar,
}

impl ScrollView{
    
    pub fn set_scroll_x(&mut self, _cx: &mut Cx, value:f32){
        self.scroll.x = value;
    }

    pub fn set_scroll_y(&mut self, _cx: &mut Cx, value:f32){
        self.scroll.y = value;
    }
    
    pub fn get_scroll_pos(&self)->Vec2{
        self.scroll
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let mut ret_x = ScrollBarEvent::None;
        let mut ret_y = ScrollBarEvent::None;
        
        if self.show_scroll_x {
            ret_x = self.scroll_bar_x.handle_event(cx, event);
        }
        if self.show_scroll_y {
            ret_y = self.scroll_bar_y.handle_event(cx, event);
        }
   
        match ret_x {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                self.set_scroll_x(cx, scroll_pos);
            },
            _ => ()
        };
        match ret_y {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                self.set_scroll_y(cx,  scroll_pos);
            },
            _ => ()
        };
        ret_x != ScrollBarEvent::None || ret_y != ScrollBarEvent::None
    }
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: Vec2) -> bool {
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if self.show_scroll_x {
            if self.scroll_bar_x.set_scroll_pos(cx, pos.x) {
                changed = true;
            }
            let scroll_pos = self.scroll_bar_x.get_scroll_pos();
            self.set_scroll_x(cx,  scroll_pos);
        }
        if self.show_scroll_y {
            if self.scroll_bar_y.set_scroll_pos(cx, pos.y) {
                changed = true;
            }
            let scroll_pos = self.scroll_bar_y.get_scroll_pos();
            self.set_scroll_y(cx,  scroll_pos);
        } 
        changed
    }
    
    pub fn set_scroll_pos_no_clip(&mut self, cx: &mut Cx, pos: Vec2)->bool {
        let mut changed = false;
        if self.show_scroll_x {
            if self.scroll_bar_x.set_scroll_pos_no_clip(cx, pos.x) {
                changed = true;
            }
            self.set_scroll_x(cx, pos.x);
        }
        if self.show_scroll_y {
            if self.scroll_bar_y.set_scroll_pos_no_clip(cx, pos.y) {
                changed = true;
            }
            self.set_scroll_y(cx, pos.y);
        } 
        changed
    }
    
    pub fn get_scroll_view_total(&mut self) -> Vec2 {
        Vec2 {
            x: if self.show_scroll_x {
                self.scroll_bar_x.get_scroll_view_total()
            }else {0.},
            y: if self.show_scroll_y {
                self.scroll_bar_y.get_scroll_view_total()
            }else {0.}
        }
    }
    
    pub fn get_scroll_view_visible(&mut self) -> Vec2 {
        Vec2 {
            x: if self.show_scroll_x {
                self.scroll_bar_x.get_scroll_view_visible()
            }else {0.},
            y: if self.show_scroll_y {
                self.scroll_bar_y.get_scroll_view_visible()
            }else {0.}
        }
    }

    pub fn get_viewport_rect(&mut self, _cx: &mut Cx)->Rect {
        let pos = self.get_scroll_pos();
        let size = self.get_scroll_view_visible();
        Rect{pos, size}
    }
    
    pub fn scroll_into_view(&mut self, cx: &mut Cx, rect: Rect) {
        if self.show_scroll_x {
            self.scroll_bar_x.scroll_into_view(cx, rect.pos.x, rect.size.x, true);
        }
        if self.show_scroll_y{
            self.scroll_bar_y.scroll_into_view(cx, rect.pos.y, rect.size.y, true);
        }
    }
    
    pub fn scroll_into_view_no_smooth(&mut self, cx: &mut Cx, rect: Rect) {
        if self.show_scroll_x {
            self.scroll_bar_x.scroll_into_view(cx, rect.pos.x, rect.size.x, false);
        }
        if self.show_scroll_y {
            self.scroll_bar_y.scroll_into_view(cx, rect.pos.y, rect.size.y, false);
        }
    }
    
    pub fn scroll_into_view_abs(&mut self, cx: &mut Cx, rect: Rect) {
        let self_rect = self.get_rect(cx); 
        if self.show_scroll_x {
            self.scroll_bar_x.scroll_into_view(cx, rect.pos.x - self_rect.pos.x, rect.size.x, true);
        }
        if self.show_scroll_y {
            self.scroll_bar_y.scroll_into_view(cx, rect.pos.y  - self_rect.pos.y, rect.size.y, true);
        }
    }
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, pos: Vec2) {
        if self.show_scroll_x {
            self.scroll_bar_x.set_scroll_target(cx, pos.x);
        }
        if self.show_scroll_y {
           self.scroll_bar_y.set_scroll_target(cx, pos.y);
        }
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) -> Area {

        let draw_list_id = self.view.draw_list_id();
        let view_area = Area::DrawList(DrawListArea {draw_list_id, redraw_id: cx.redraw_id()});
        
        // lets ask the turtle our actual bounds
        let view_total = cx.turtle().used();
        let mut rect_now = cx.turtle().rect();
        if rect_now.size.y.is_nan() {
            rect_now.size.y = view_total.y;
        }
        if rect_now.size.x.is_nan() {
            rect_now.size.x = view_total.x;
        }
        
        if self.show_scroll_x {
            let scroll_pos = self.scroll_bar_x.draw_scroll_bar(cx, Axis::Horizontal, view_area, rect_now, view_total);
            self.set_scroll_x(cx, scroll_pos);
        }
        if self.show_scroll_y {
            //println!("SET SCROLLBAR {} {}", rect_now.h, view_total.y);
            let scroll_pos = self.scroll_bar_y.draw_scroll_bar(cx, Axis::Vertical, view_area, rect_now, view_total);
            self.set_scroll_y(cx, scroll_pos);
        }
        
        let rect = cx.end_turtle_with_guard(view_area);
        
        cx.set_view_rect(draw_list_id, rect);
        
        cx.draw_list_stack.pop();
        
        return view_area
    }
}
