use render::*;
use crate::scrollbar::*;

#[derive(Clone)]
pub struct ScrollView{
    pub view:View,
    pub scroll_h:Option<ScrollBar>,
    pub scroll_v:Option<ScrollBar>,
}

impl ScrollView{

    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            view: View::proto(cx),
            scroll_h: Some(ScrollBar::proto(cx)),
            scroll_v: Some(ScrollBar {
                smoothing: Some(0.15),
                ..ScrollBar::proto(cx)
            }),
        }
    }
    
    pub fn proto_no_scroll(cx: &mut Cx) -> Self {
        Self {
            view: View::proto(cx),
            scroll_h: None,
            scroll_v: None
        }
    }
    
    pub fn begin_view(&mut self, cx: &mut Cx, layout: Layout) -> ViewRedraw {
        self.view.begin_view(cx, layout)
    }
    
    pub fn view_will_redraw(&mut self, cx: &mut Cx)->bool{
        self.view.view_will_redraw(cx)
    }
    
    pub fn handle_scroll_bars(&mut self, cx: &mut Cx, event: &mut Event) -> bool {
        let mut ret_h = ScrollBarEvent::None;
        let mut ret_v = ScrollBarEvent::None;
        
        if let Some(scroll_h) = &mut self.scroll_h {
            ret_h = scroll_h.handle_scroll_bar(cx, event);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            ret_v = scroll_v.handle_scroll_bar(cx, event);
        }
        match ret_h {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                cx.set_view_scroll_x(self.view.view_id.unwrap(), scroll_pos);
            },
            _ => ()
        };
        match ret_v {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                cx.set_view_scroll_y(self.view.view_id.unwrap(), scroll_pos);
            },
            _ => ()
        };
        ret_h != ScrollBarEvent::None || ret_v != ScrollBarEvent::None
    }
    
    pub fn get_scroll_pos(&self, cx: &Cx) -> Vec2 {
        if let Some(view_id) = self.view.view_id {
            let cxview = &cx.views[view_id];
            cxview.unsnapped_scroll
        }
        else {
            Vec2::zero()
        }
    }
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: Vec2) -> bool {
        let view_id = self.view.view_id.unwrap();
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if let Some(scroll_h) = &mut self.scroll_h {
            if scroll_h.set_scroll_pos(cx, pos.x) {
                let scroll_pos = scroll_h.get_scroll_pos();
                cx.set_view_scroll_x(view_id, scroll_pos);
                changed = true;
            }
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            if scroll_v.set_scroll_pos(cx, pos.y) {
                let scroll_pos = scroll_v.get_scroll_pos();
                cx.set_view_scroll_y(view_id, scroll_pos);
                changed = true;
            }
        }
        changed
    }
    
    pub fn set_scroll_view_total(&mut self, cx: &mut Cx, view_total: Vec2) {
        if let Some(scroll_h) = &mut self.scroll_h {
            scroll_h.set_scroll_view_total(cx, view_total.x)
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            scroll_v.set_scroll_view_total(cx, view_total.y)
        }
    }
    
    pub fn get_scroll_view_total(&mut self) -> Vec2 {
        Vec2 {
            x: if let Some(scroll_h) = &mut self.scroll_h {
                scroll_h.get_scroll_view_total()
            }else {0.},
            y: if let Some(scroll_v) = &mut self.scroll_v {
                scroll_v.get_scroll_view_total()
            }else {0.}
        }
    }
    
    pub fn scroll_into_view(&mut self, cx: &mut Cx, rect: Rect) {
        if let Some(scroll_h) = &mut self.scroll_h {
            scroll_h.scroll_into_view(cx, rect.x, rect.w);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            scroll_v.scroll_into_view(cx, rect.y, rect.h);
        }
    }
    
    pub fn scroll_into_view_abs(&mut self, cx: &mut Cx, rect: Rect) {
        let self_rect = self.get_rect(cx); 
        if let Some(scroll_h) = &mut self.scroll_h {
            scroll_h.scroll_into_view(cx, rect.x - self_rect.x, rect.w);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            scroll_v.scroll_into_view(cx, rect.y  - self_rect.y, rect.h);
        }
    }
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, pos: Vec2) {
        if let Some(scroll_h) = &mut self.scroll_h {
            scroll_h.set_scroll_target(cx, pos.x);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            scroll_v.set_scroll_target(cx, pos.y);
        }
    }
    
    pub fn end_view(&mut self, cx: &mut Cx) -> Area {

        let view_id = self.view.view_id.unwrap();
        let view_area = Area::View(ViewArea {view_id: view_id, redraw_id: cx.redraw_id});
        
        // lets ask the turtle our actual bounds
        let view_total = cx.get_turtle_bounds();
        let mut rect_now = cx.get_turtle_rect();
        if rect_now.h.is_nan() {
            rect_now.h = view_total.y;
        }
        if rect_now.w.is_nan() {
            rect_now.w = view_total.x;
        }
        
        if let Some(scroll_h) = &mut self.scroll_h {
            let scroll_pos = scroll_h.draw_scroll_bar(cx, Axis::Horizontal, view_area, rect_now, view_total);
            cx.set_view_scroll_x(view_id, scroll_pos);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            //println!("SET SCROLLBAR {} {}", rect_now.h, view_total.y);
            let scroll_pos = scroll_v.draw_scroll_bar(cx, Axis::Vertical, view_area, rect_now, view_total);
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
    
    
    pub fn redraw_view_area(&self, cx: &mut Cx) {
        self.view.redraw_view_area(cx)
    }
    
    pub fn get_view_area(&self, cx: &Cx) -> Area {
        self.view.get_view_area(cx)
    }
}