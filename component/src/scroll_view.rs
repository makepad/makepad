use crate::{
    makepad_platform::*,
    scroll_bar::*
};

live_register!{
    ScrollView:{{ScrollView}}{
        h_show:true,
        v_show:true,
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
    pub h_show:bool,
    pub v_show:bool,
    pub h_scroll:ScrollBar,
    pub v_scroll:ScrollBar,
}

impl ScrollView{
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> bool {
        let mut ret_h = ScrollBarEvent::None;
        let mut ret_v = ScrollBarEvent::None;
        
        if self.h_show {
            ret_h = self.h_scroll.handle_event(cx, event);
        }
        if self.v_show {
            ret_v = self.v_scroll.handle_event(cx, event);
        }
   
        match ret_h {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                cx.set_view_scroll_x(self.view.draw_list_id, scroll_pos);
            },
            _ => ()
        };
        match ret_v {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                cx.set_view_scroll_y(self.view.draw_list_id, scroll_pos);
            },
            _ => ()
        };
        ret_h != ScrollBarEvent::None || ret_v != ScrollBarEvent::None
    }
    
    pub fn get_scroll_pos(&self, cx: &Cx) -> Vec2 {
        let draw_list = &cx.draw_lists[self.view.draw_list_id];
        draw_list.unsnapped_scroll
    }
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: Vec2) -> bool {
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if self.h_show {
            if self.h_scroll.set_scroll_pos(cx, pos.x) {
                changed = true;
            }
            let scroll_pos = self.h_scroll.get_scroll_pos();
            cx.set_view_scroll_x(self.view.draw_list_id, scroll_pos);
        }
        if self.v_show {
            if self.v_scroll.set_scroll_pos(cx, pos.y) {
                changed = true;
            }
            let scroll_pos = self.v_scroll.get_scroll_pos();
            cx.set_view_scroll_y(self.view.draw_list_id, scroll_pos);
        } 
        changed
    }
    
    pub fn set_scroll_pos_no_clip(&mut self, cx: &mut Cx, pos: Vec2)->bool {
        let mut changed = false;
        if self.h_show {
            if self.h_scroll.set_scroll_pos_no_clip(cx, pos.x) {
                changed = true;
            }
            cx.set_view_scroll_x(self.view.draw_list_id, pos.x);
        }
        if self.v_show {
            if self.v_scroll.set_scroll_pos_no_clip(cx, pos.y) {
                changed = true;
            }
            cx.set_view_scroll_y(self.view.draw_list_id, pos.y);
        } 
        changed
    }
    
    pub fn get_scroll_view_total(&mut self) -> Vec2 {
        Vec2 {
            x: if self.h_show {
                self.h_scroll.get_scroll_view_total()
            }else {0.},
            y: if self.v_show {
                self.v_scroll.get_scroll_view_total()
            }else {0.}
        }
    }
    
    pub fn get_scroll_view_visible(&mut self) -> Vec2 {
        Vec2 {
            x: if self.h_show {
                self.h_scroll.get_scroll_view_visible()
            }else {0.},
            y: if self.v_show {
                self.v_scroll.get_scroll_view_visible()
            }else {0.}
        }
    }

    pub fn get_viewport_rect(&mut self, cx: &mut Cx)->Rect {
        let pos = self.get_scroll_pos(cx);
        let size = self.get_scroll_view_visible();
        Rect{pos, size}
    }
    
    pub fn scroll_into_view(&mut self, cx: &mut Cx, rect: Rect) {
        if self.h_show {
            self.h_scroll.scroll_into_view(cx, rect.pos.x, rect.size.x, true);
        }
        if self.v_show{
            self.v_scroll.scroll_into_view(cx, rect.pos.y, rect.size.y, true);
        }
    }
    
    pub fn scroll_into_view_no_smooth(&mut self, cx: &mut Cx, rect: Rect) {
        if self.h_show {
            self.h_scroll.scroll_into_view(cx, rect.pos.x, rect.size.x, false);
        }
        if self.v_show {
            self.v_scroll.scroll_into_view(cx, rect.pos.y, rect.size.y, false);
        }
    }
    
    pub fn scroll_into_view_abs(&mut self, cx: &mut Cx, rect: Rect) {
        let self_rect = self.get_rect(cx); 
        if self.h_show {
            self.h_scroll.scroll_into_view(cx, rect.pos.x - self_rect.pos.x, rect.size.x, true);
        }
        if self.v_show {
            self.v_scroll.scroll_into_view(cx, rect.pos.y  - self_rect.pos.y, rect.size.y, true);
        }
    }
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, pos: Vec2) {
        if self.h_show {
            self.h_scroll.set_scroll_target(cx, pos.x);
        }
        if self.v_show {
           self.v_scroll.set_scroll_target(cx, pos.y);
        }
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) -> Area {

        let draw_list_id = self.view.draw_list_id;
        let view_area = Area::DrawList(DrawListArea {draw_list_id, redraw_id: cx.redraw_id});
        
        // lets ask the turtle our actual bounds
        let view_total = cx.get_turtle_bounds();
        let mut rect_now = cx.get_turtle_rect();
        if rect_now.size.y.is_nan() {
            rect_now.size.y = view_total.y;
        }
        if rect_now.size.x.is_nan() {
            rect_now.size.x = view_total.x;
        }
        
        if self.h_show {
            let scroll_pos = self.h_scroll.draw_scroll_bar(cx, Axis::Horizontal, view_area, rect_now, view_total);
            cx.set_view_scroll_x(draw_list_id, scroll_pos);
        }
        if self.v_show {
            //println!("SET SCROLLBAR {} {}", rect_now.h, view_total.y);
            let scroll_pos = self.v_scroll.draw_scroll_bar(cx, Axis::Vertical, view_area, rect_now, view_total);
            cx.set_view_scroll_y(draw_list_id, scroll_pos);
        }
        
        let rect = cx.end_turtle_with_guard(view_area);
        let draw_list = &mut cx.draw_lists[draw_list_id];
        draw_list.rect = rect;
        cx.draw_list_stack.pop();
        
        return view_area
    }
}
