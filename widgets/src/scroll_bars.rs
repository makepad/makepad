use crate::{
    makepad_draw::*,
    scroll_bar::*
};

live_design!{
    ScrollBarsBase = {{ScrollBars}} {}
}

#[derive(Live, LiveHook, LiveRegister)]
pub struct ScrollBars {
    #[live] show_scroll_x: bool,
    #[live] show_scroll_y: bool,
    #[live] scroll_bar_x: ScrollBar,
    #[live] scroll_bar_y: ScrollBar,
    #[rust] nav_scroll_index: Option<NavScrollIndex>,
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

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope)->Vec<ScrollBarsAction> {
        let mut actions = Vec::new();
        self.handle_main_event(cx, event, scope, &mut actions);
        self.handle_scroll_event(cx, event, scope, &mut actions);
        actions
    }
    
    pub fn handle_main_event(&mut self, cx: &mut Cx, event: &Event, _scope:&mut Scope, actions: &mut Vec<ScrollBarsAction> ) {
        if let Event::Trigger(te) = event{
            if let Some(triggers) = te.triggers.get(&self.area){
                if let Some(trigger) = triggers.iter().find(|t| t.id == live_id!(scroll_focus_nav)){
                    let self_rect = self.area.rect(cx);
                    self.scroll_into_view(
                        cx,
                        trigger.from.rect(cx)
                        .translate(-self_rect.pos + self.scroll)
                        .add_margin(dvec2(5.0,5.0))
                    );
                }
            }
        }
        
        if self.show_scroll_x {
            let mut ret_x = None;
            self.scroll_bar_x.handle_event_with(cx, event, &mut | _cx, action | {
                match action {
                    ScrollBarAction::Scroll {scroll_pos, ..} => {
                        ret_x = Some(scroll_pos);
                        actions.push(ScrollBarsAction::ScrollX(scroll_pos))
                    }
                    _ => ()
                }
            });
            if let Some(x) = ret_x {self.scroll.x = x; self.redraw(cx);}
        }
        if self.show_scroll_y {
            let mut ret_y = None;
            self.scroll_bar_y.handle_event_with(cx, event, &mut | _cx, action | {
                match action {
                    ScrollBarAction::Scroll {scroll_pos, ..} => {
                        ret_y = Some(scroll_pos);
                        actions.push(ScrollBarsAction::ScrollY(scroll_pos))
                    }
                    _ => ()
                }
            });
            if let Some(y) = ret_y {self.scroll.y = y; self.redraw(cx);}
        }
    }
    
    pub fn handle_scroll_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope, actions: &mut Vec<ScrollBarsAction> ) {
        
        if self.show_scroll_x {
            let mut ret_x = None;
            self.scroll_bar_x.handle_scroll_event(cx, event, self.area, &mut | _cx, action | {
                match action {
                    ScrollBarAction::Scroll {scroll_pos, ..} => {
                        ret_x = Some(scroll_pos);
                        actions.push(ScrollBarsAction::ScrollX(scroll_pos))
                    }
                    _ => ()
                }
            });
            if let Some(x) = ret_x {self.scroll.x = x; self.redraw(cx);}
        }
        if self.show_scroll_y {
            let mut ret_y = None;
            self.scroll_bar_y.handle_scroll_event(cx, event, self.area, &mut | _cx, action | {
                match action {
                    ScrollBarAction::Scroll {scroll_pos, ..} => {
                        ret_y = Some(scroll_pos);
                        actions.push(ScrollBarsAction::ScrollY(scroll_pos))
                    }
                    _ => ()
                }
            });
            if let Some(y) = ret_y {self.scroll.y = y; self.redraw(cx);}
        }
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
        let self_rect = self.area.rect(cx);
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
    
    
    // all in one scrollbar api
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk, layout: Layout) {
        cx.begin_turtle(walk, layout.with_scroll(self.scroll));
        self.begin_nav_area(cx);
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        self.draw_scroll_bars(cx);
        // this needs to be a rect_area
        cx.end_turtle_with_area(&mut self.area);
        self.end_nav_area(cx);
    }
    
    
    pub fn end_with_shift(&mut self, cx: &mut Cx2d) {
        self.draw_scroll_bars(cx);
        // this needs to be a rect_area
        cx.end_turtle_with_area(&mut self.area);
        self.end_nav_area(cx);
    }
    // separate API
    
    pub fn begin_nav_area(&mut self, cx: &mut Cx2d) {
        self. nav_scroll_index = Some(cx.add_begin_scroll());
    }
    
    pub fn end_nav_area(&mut self, cx: &mut Cx2d) {
        if !self.area.is_valid(cx) {
            error!("Call set area before end_nav_area");
            return
        }
        cx.add_end_scroll(self.nav_scroll_index.take().unwrap(), self.area);
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
            let scroll_pos = self.scroll_bar_x.draw_scroll_bar(cx, ScrollAxis::Horizontal, rect_now, view_total);
            self.set_scroll_x(cx, scroll_pos);
        }
        if self.show_scroll_y {
            //println!("SET SCROLLBAR {} {}", rect_now.h, view_total.y);
            let scroll_pos = self.scroll_bar_y.draw_scroll_bar(cx, ScrollAxis::Vertical, rect_now, view_total);
            self.set_scroll_y(cx, scroll_pos);
        }
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