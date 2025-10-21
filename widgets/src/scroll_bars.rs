use crate::{
    makepad_draw::*,
    scroll_bar::*
};

live_design!{
    link widgets;
    use link::theme::*;
    use link::widgets::*;
    use makepad_draw::shader::std::*;
    
    pub ScrollBarsBase = {{ScrollBars}} {}
    
    pub ScrollBarsTabs = <ScrollBarsBase> {
        show_scroll_x: true,
        show_scroll_y: true,
        scroll_bar_x: <ScrollBarTabs> {}
        scroll_bar_y: <ScrollBarTabs> {}
    }
    
    pub ScrollBars = <ScrollBarsBase> {
        show_scroll_x: true,
        show_scroll_y: true,
        scroll_bar_x: <ScrollBar> {}
        scroll_bar_y: <ScrollBar> {}
    }
}

/// A sample of a scroll event
#[derive(Clone, Copy)]
struct ScrollSample {
    abs: DVec2,
    time: f64,
}

enum ScrollState {
    Stopped,
    Drag { samples: Vec<ScrollSample> },
    Flick { delta: DVec2, next_frame: NextFrame },
}

#[derive(Live, LiveHook, LiveRegister)]
pub struct ScrollBars {
    #[live] show_scroll_x: bool,
    #[live] show_scroll_y: bool,
    #[live] scroll_bar_x: ScrollBar,
    #[live] scroll_bar_y: ScrollBar,

    /// The minimum amount of scroll to trigger a flick animation
    #[live(0.2)] flick_scroll_minimum: f64,
    /// The maximum amount of scroll to trigger a flick animation
    #[live(80.0)] flick_scroll_maximum: f64,
    /// The scaling factor for the flick animation
    #[live(0.005)] flick_scroll_scaling: f64,
    /// The decay factor for the flick animation
    #[live(0.97)] flick_scroll_decay: f64,
    /// Whether to enable drag scrolling
    #[live(true)] drag_scrolling: bool,

    #[rust] nav_scroll_index: Option<NavScrollIndex>,
    #[rust] scroll: DVec2,
    #[rust] area: Area,
    #[rust(ScrollState::Stopped)] scroll_state: ScrollState,
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
        self.handle_flick(cx, event, actions);

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

        self.handle_touch_based_drag(cx, event, actions);
    }

    fn handle_flick(&mut self, cx: &mut Cx, event: &Event, actions: &mut Vec<ScrollBarsAction> ) {
        let flick_delta = if let ScrollState::Flick { delta, next_frame } = &self.scroll_state {
            if next_frame.is_event(event).is_some() {
                Some(*delta)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(mut delta) = flick_delta {
            delta = delta * self.flick_scroll_decay;

            let mut should_continue = false;
            if self.show_scroll_x && delta.x.abs() > self.flick_scroll_minimum {
                should_continue = true;
            }
            if self.show_scroll_y && delta.y.abs() > self.flick_scroll_minimum {
                should_continue = true;
            }

            if should_continue {
                let mut pos = self.scroll;
                if self.show_scroll_x {
                    pos.x -= delta.x;
                }
                if self.show_scroll_y {
                    pos.y -= delta.y;
                }

                if self.set_scroll_pos(cx, pos) {
                    if self.show_scroll_x && delta.x.abs() > 0.0 {
                        actions.push(ScrollBarsAction::ScrollX(pos.x));
                    }
                    if self.show_scroll_y && delta.y.abs() > 0.0 {
                        actions.push(ScrollBarsAction::ScrollY(pos.y));
                    }
                }

                self.scroll_state = ScrollState::Flick {
                    delta,
                    next_frame: cx.new_next_frame()
                };
            } else {
                self.scroll_state = ScrollState::Stopped;
            }
        }
    }
    
    /// Handles touch-based drag scrolling
    fn handle_touch_based_drag(&mut self, cx: &mut Cx, event: &Event, actions: &mut Vec<ScrollBarsAction> ) {
        if !self.drag_scrolling {
            return;
        }

        // Check if scroll bar handles are not captured
        let scroll_bar_captured = self.scroll_bar_x.is_area_captured(cx)
            || self.scroll_bar_y.is_area_captured(cx);

        if scroll_bar_captured {
            self.scroll_state = ScrollState::Stopped;
            return;
        }

        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.scroll_state = ScrollState::Drag {
                    samples: vec![ScrollSample { abs: fe.abs, time: fe.time }]
                };
            }
            Hit::FingerMove(e) => {
                match &mut self.scroll_state {
                    ScrollState::Drag { samples } => {
                        let new_abs = e.abs;
                        let old_sample = *samples.last().unwrap();
                        samples.push(ScrollSample { abs: new_abs, time: e.time });
                        if samples.len() > 4 {
                            samples.remove(0);
                        }

                        let delta = new_abs - old_sample.abs;
                        let mut pos = self.scroll;

                        if self.show_scroll_x {
                            pos.x -= delta.x;
                        }
                        if self.show_scroll_y {
                            pos.y -= delta.y;
                        }

                        if self.set_scroll_pos(cx, pos) {
                            if self.show_scroll_x && delta.x.abs() > 0.0 {
                                actions.push(ScrollBarsAction::ScrollX(pos.x));
                            }
                            if self.show_scroll_y && delta.y.abs() > 0.0 {
                                actions.push(ScrollBarsAction::ScrollY(pos.y));
                            }
                        }
                    }
                    _ => ()
                }
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                match &mut self.scroll_state {
                    ScrollState::Drag { samples } => {
                        let mut last = None;
                        let mut scaled_delta = DVec2::default();
                        let mut total_delta = DVec2::default();

                        for sample in samples.iter().rev() {
                            if last.is_none() {
                                last = Some(sample);
                            } else {
                                let time_delta = last.unwrap().time - sample.time;
                                if time_delta > 0.0 {
                                    let abs_delta = last.unwrap().abs - sample.abs;
                                    total_delta += abs_delta;
                                    scaled_delta += abs_delta / time_delta;
                                }
                            }
                        }

                        scaled_delta *= self.flick_scroll_scaling;

                        // Check if we should start flick animation
                        let mut should_flick = false;
                        if self.show_scroll_x && total_delta.x.abs() > 10.0 && scaled_delta.x.abs() > self.flick_scroll_minimum {
                            should_flick = true;
                        }
                        if self.show_scroll_y && total_delta.y.abs() > 10.0 && scaled_delta.y.abs() > self.flick_scroll_minimum {
                            should_flick = true;
                        }

                        if should_flick {
                            // Clamp the delta to maximum values
                            let delta = DVec2 {
                                x: scaled_delta.x.min(self.flick_scroll_maximum).max(-self.flick_scroll_maximum),
                                y: scaled_delta.y.min(self.flick_scroll_maximum).max(-self.flick_scroll_maximum),
                            };
                            self.scroll_state = ScrollState::Flick {
                                delta,
                                next_frame: cx.new_next_frame()
                            };
                        } else {
                            self.scroll_state = ScrollState::Stopped;
                        }
                    }
                    _ => ()
                }
            }
            _ => ()
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