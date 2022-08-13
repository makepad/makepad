use crate::{
    makepad_draw_2d::*,
    scroll_bar::*
};

live_register!{
    ScrollBars: {{ScrollBars}} {
        show_scroll_x: true,
        show_scroll_y: true,
    }
}

#[derive(Live, LiveHook)]
pub struct ScrollBars {
    show_scroll_x: bool,
    show_scroll_y: bool,
    scroll_bar_x: ScrollBar,
    scroll_bar_y: ScrollBar,
    #[rust] scroll: DVec2,
    #[rust] area: Area,
}

pub enum ScrollBarsAction {
    ScrollX(f64),
    ScrollY(f64),
    None
}

impl ScrollBars {
    
    pub fn set_scroll_x(&mut self, _cx: &mut Cx, value: f64) {
        self.scroll.x = value;
    }
    
    pub fn set_scroll_y(&mut self, _cx: &mut Cx, value: f64) {
        self.scroll.y = value;
    }
    
    pub fn get_scroll_pos(&self) -> DVec2 {
        self.scroll
    }
    
    fn handle_internal(&mut self, cx: &mut Cx, event: &Event, is_scroll: bool, dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarsAction)) {
        let mut ret_x = None;
        let mut ret_y = None;
        let area = if is_scroll {Some(self.area)}else {None};
        if self.show_scroll_x {
            self.scroll_bar_x.handle_event(cx, event, area, &mut | cx, action | {
                match action {
                    ScrollBarAction::Scroll {scroll_pos, ..} => {
                        ret_x = Some(scroll_pos);
                        dispatch_action(cx, ScrollBarsAction::ScrollX(scroll_pos))
                    }
                    _ => ()
                }
            });
            if let Some(x) = ret_x {self.scroll.x = x; self.redraw(cx);}
        }
        if self.show_scroll_y {
            self.scroll_bar_y.handle_event(cx, event, area, &mut | cx, action | {
                match action {
                    ScrollBarAction::Scroll {scroll_pos, ..} => {
                        ret_y = Some(scroll_pos);
                        dispatch_action(cx, ScrollBarsAction::ScrollY(scroll_pos))
                    }
                    _ => ()
                }
            });
            if let Some(y) = ret_y {self.scroll.y = y; self.redraw(cx);}
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarsAction)) {
        self.handle_internal(cx, event, false, dispatch_action);
    }
    
    pub fn handle_scroll(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarsAction)) {
        self.handle_internal(cx, event, true, dispatch_action);
    }
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: DVec2) -> bool {
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if self.show_scroll_x {
            if self.scroll_bar_x.set_scroll_pos(cx, pos.x) {
                changed = true;
            }
            let scroll_pos = self.scroll_bar_x.get_scroll_pos();
            self.set_scroll_x(cx, scroll_pos);
        }
        if self.show_scroll_y {
            if self.scroll_bar_y.set_scroll_pos(cx, pos.y) {
                changed = true;
            }
            let scroll_pos = self.scroll_bar_y.get_scroll_pos();
            self.set_scroll_y(cx, scroll_pos);
        }
        changed
    }
    
    pub fn set_scroll_pos_no_clip(&mut self, cx: &mut Cx, pos: DVec2) -> bool {
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
    
    pub fn get_scroll_view_total(&mut self) -> DVec2 {
        DVec2 {
            x: if self.show_scroll_x {
                self.scroll_bar_x.get_scroll_view_total()
            }else {0.},
            y: if self.show_scroll_y {
                self.scroll_bar_y.get_scroll_view_total()
            }else {0.}
        }
    }
    
    pub fn get_scroll_view_visible(&mut self) -> DVec2 {
        DVec2 {
            x: if self.show_scroll_x {
                self.scroll_bar_x.get_scroll_view_visible()
            }else {0.},
            y: if self.show_scroll_y {
                self.scroll_bar_y.get_scroll_view_visible()
            }else {0.}
        }
    }
    
    pub fn get_viewport_rect(&mut self, _cx: &mut Cx) -> Rect {
        let pos = self.get_scroll_pos();
        let size = self.get_scroll_view_visible();
        Rect {pos, size}
    }
    
    pub fn scroll_into_view(&mut self, cx: &mut Cx, rect: Rect) {
        if self.show_scroll_x {
            self.scroll_bar_x.scroll_into_view(cx, rect.pos.x, rect.size.x, true);
        }
        if self.show_scroll_y {
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
        let self_rect = self.area.get_rect(cx);
        if self.show_scroll_x {
            self.scroll_bar_x.scroll_into_view(cx, rect.pos.x - self_rect.pos.x, rect.size.x, true);
        }
        if self.show_scroll_y {
            self.scroll_bar_y.scroll_into_view(cx, rect.pos.y - self_rect.pos.y, rect.size.y, true);
        }
    }
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, pos: DVec2) {
        if self.show_scroll_x {
            self.scroll_bar_x.set_scroll_target(cx, pos.x);
        }
        if self.show_scroll_y {
            self.scroll_bar_y.set_scroll_target(cx, pos.y);
        }
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk, layout: Layout) {
        cx.begin_turtle(walk, layout.with_scroll(self.scroll));
    }
    
    pub fn draw_scroll_bars(&mut self, cx: &mut Cx2d) {
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
            let scroll_pos = self.scroll_bar_x.draw_scroll_bar(cx, Axis::Horizontal, rect_now, view_total);
            self.set_scroll_x(cx, scroll_pos);
        }
        if self.show_scroll_y {
            //println!("SET SCROLLBAR {} {}", rect_now.h, view_total.y);
            let scroll_pos = self.scroll_bar_y.draw_scroll_bar(cx, Axis::Vertical, rect_now, view_total);
            self.set_scroll_y(cx, scroll_pos);
        }
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        self.draw_scroll_bars(cx);
        // this needs to be a rect_area
        cx.end_turtle_with_area(&mut self.area);
    }
    
    pub fn set_area(&mut self, area: Area) {
        self.area = area;
    }
    
    pub fn area(&self) -> Area {
        self.area
    }
    
    pub fn redraw(&self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}
