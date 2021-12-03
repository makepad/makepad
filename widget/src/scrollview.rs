use makepad_render::*;
use crate::scrollbar::*;

live_register!{
    ScrollView:{{ScrollView}}{
        show_h:true,
        show_v:true,
        view:{debug_id:scroll_inner_view}
    }
}

#[derive(Live, LiveHook)]
pub struct ScrollView{
    pub view:View,
    pub show_h:bool,
    pub show_v:bool,
    pub scroll_h:ScrollBar,
    pub scroll_v:ScrollBar,
}

impl ScrollView{
/*
    pub fn new() -> Self {
        Self {
            view: View::new(),
            scroll_h: None,
            scroll_v: None
        }
    }

    pub fn new_standard_hv(cx: &mut Cx) -> Self {
        Self {
            view: View::new(),
            scroll_h: Some(ScrollBar::new(cx)),
            scroll_v: Some(ScrollBar::new(cx)
                .with_smoothing(0.15)),
        }
    }
   
    pub fn with_scroll_h(self, s:ScrollBar)->Self{
        Self{scroll_h:Some(s), ..self}
    }
      
    pub fn with_scroll_v(self, s:ScrollBar)->Self{
        Self{scroll_v:Some(s), ..self}
    }
    */
    pub fn begin(&mut self, cx: &mut Cx) -> ViewRedraw {
        self.view.begin(cx)
    }
    
    pub fn view_will_redraw(&mut self, cx: &mut Cx)->bool{
        self.view.view_will_redraw(cx)
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> bool {
        let mut ret_h = ScrollBarEvent::None;
        let mut ret_v = ScrollBarEvent::None;
        
        if self.show_h {
            ret_h = self.scroll_h.handle_event(cx, event);
        }
        if self.show_v {
            ret_v = self.scroll_v.handle_event(cx, event);
        }
   
        match ret_h {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                cx.set_view_scroll_x(self.view.view_id, scroll_pos);
            },
            _ => ()
        };
        match ret_v {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                cx.set_view_scroll_y(self.view.view_id, scroll_pos);
            },
            _ => ()
        };
        ret_h != ScrollBarEvent::None || ret_v != ScrollBarEvent::None
    }
    
    pub fn get_scroll_pos(&self, cx: &Cx) -> Vec2 {
        let cxview = &cx.views[self.view.view_id];
        cxview.unsnapped_scroll
    }
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: Vec2) -> bool {
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if self.show_h {
            if self.scroll_h.set_scroll_pos(cx, pos.x) {
                let scroll_pos = self.scroll_h.get_scroll_pos();
                cx.set_view_scroll_x(self.view.view_id, scroll_pos);
                changed = true;
            }
        }
        if self.show_v {
            if self.scroll_v.set_scroll_pos(cx, pos.y) {
                let scroll_pos = self.scroll_v.get_scroll_pos();
                cx.set_view_scroll_y(self.view.view_id, scroll_pos);
                changed = true;
            }
        }
        changed
    }
    
    
    pub fn get_scroll_view_total(&mut self) -> Vec2 {
        Vec2 {
            x: if self.show_h {
                self.scroll_h.get_scroll_view_total()
            }else {0.},
            y: if self.show_v {
                self.scroll_v.get_scroll_view_total()
            }else {0.}
        }
    }
    
    pub fn scroll_into_view(&mut self, cx: &mut Cx, rect: Rect) {
        if self.show_h {
            self.scroll_h.scroll_into_view(cx, rect.pos.x, rect.size.x, true);
        }
        if self.show_v{
            self.scroll_v.scroll_into_view(cx, rect.pos.y, rect.size.y, true);
        }
    }
    
    pub fn scroll_into_view_no_smooth(&mut self, cx: &mut Cx, rect: Rect) {
        if self.show_h {
            self.scroll_h.scroll_into_view(cx, rect.pos.x, rect.size.x, false);
        }
        if self.show_v {
            self.scroll_v.scroll_into_view(cx, rect.pos.y, rect.size.y, false);
        }
    }
    
    pub fn scroll_into_view_abs(&mut self, cx: &mut Cx, rect: Rect) {
        let self_rect = self.get_rect(cx); 
        if self.show_h {
            self.scroll_h.scroll_into_view(cx, rect.pos.x - self_rect.pos.x, rect.size.x, true);
        }
        if self.show_v {
            self.scroll_v.scroll_into_view(cx, rect.pos.y  - self_rect.pos.y, rect.size.y, true);
        }
    }
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, pos: Vec2) {
        if self.show_h {
            self.scroll_h.set_scroll_target(cx, pos.x);
        }
        if self.show_v {
           self.scroll_v.set_scroll_target(cx, pos.y);
        }
    }
    
    pub fn end(&mut self, cx: &mut Cx) -> Area {

        let view_id = self.view.view_id;
        let view_area = Area::View(ViewArea {view_id: view_id, redraw_id: cx.redraw_id});
        
        // lets ask the turtle our actual bounds
        let view_total = cx.get_turtle_bounds();
        let mut rect_now = cx.get_turtle_rect();
        if rect_now.size.y.is_nan() {
            rect_now.size.y = view_total.y;
        }
        if rect_now.size.x.is_nan() {
            rect_now.size.x = view_total.x;
        }
        
        if self.show_h {
            let scroll_pos = self.scroll_h.draw_scroll_bar(cx, Axis::Horizontal, view_area, rect_now, view_total);
            cx.set_view_scroll_x(view_id, scroll_pos);
        }
        if self.show_v {
            //println!("SET SCROLLBAR {} {}", rect_now.h, view_total.y);
            let scroll_pos = self.scroll_v.draw_scroll_bar(cx, Axis::Vertical, view_area, rect_now, view_total);
            cx.set_view_scroll_y(view_id, scroll_pos);
        }
        
        let rect = cx.end_turtle(view_area);
        let cxview = &mut cx.views[view_id];
        cxview.rect = rect;
        cx.view_stack.pop();
        
        return view_area
    }
    
    pub fn get_rect(&mut self, cx: &Cx) -> Rect {
        self.view.get_rect(cx)
    }
    
    
    pub fn redraw(&self, cx: &mut Cx) {
        self.view.redraw(cx)
    }
    
    pub fn area(&self) -> Area {
        self.view.area()
    }
}
