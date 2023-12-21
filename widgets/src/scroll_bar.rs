use crate::makepad_draw::*;

live_design!{
    DrawScrollBar= {{DrawScrollBar}} {}
    ScrollBarBase= {{ScrollBar}} {}
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum ScrollAxis {
    #[pick] Horizontal,
    Vertical
}
#[derive(Live, LiveHook, LiveRegister)]
pub struct ScrollBar {
    #[live] draw_bar: DrawScrollBar,
    #[live] pub bar_size: f64,
    #[live] pub min_handle_size: f64, //minimum size of the handle in pixels
    #[live] bar_side_margin: f64,
    #[live(ScrollAxis::Horizontal)] pub axis: ScrollAxis,
    
    #[live] use_vertical_finger_scroll: bool,
    #[live] smoothing: Option<f64>,
    
    #[animator] animator: Animator,
    
    #[rust] next_frame: NextFrame,
    #[rust(false)] visible: bool,
    #[rust] view_total: f64, // the total view area
    #[rust] view_visible: f64, // the visible view area
    #[rust] scroll_size: f64, // the size of the scrollbar
    #[rust] scroll_pos: f64, // scrolling position non normalised
    
    #[rust] scroll_target: f64,
    #[rust] scroll_delta: f64,
    #[rust] drag_point: Option<f64>, // the point in pixels where we are dragging
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawScrollBar {
    #[deref] draw_super: DrawQuad,
    #[live] is_vertical: f32,
    #[live] norm_handle: f32,
    #[live] norm_scroll: f32
}

#[derive(Clone, PartialEq, Debug)]
pub enum ScrollBarAction {
    None,
    Scroll {scroll_pos: f64, view_total: f64, view_visible: f64},
    ScrollDone
}

impl ScrollBar {
    /*
    pub fn with_bar_size(self, bar_size: f32) -> Self {Self {bar_size, ..self}}
    pub fn with_smoothing(self, s: f32) -> Self {Self {smoothing: Some(s), ..self}}
    pub fn with_use_vertical_finger_scroll(self, use_vertical_finger_scroll: bool) -> Self {Self {use_vertical_finger_scroll, ..self}}
    */
    // reads back normalized scroll position info
    pub fn get_normalized_scroll_pos(&self) -> (f64, f64) {
        // computed handle size normalized
        let vy = self.view_visible / self.view_total;
        if !self.visible {
            return (0.0, 0.0);
        }
        let norm_handle = vy.max(self.min_handle_size / self.scroll_size);
        let norm_scroll = (1. - norm_handle) * ((self.scroll_pos / self.view_total) / (1. - vy));
        return (norm_scroll, norm_handle)
    }
    
    // sets the scroll pos from finger position
    pub fn set_scroll_pos_from_finger(&mut self,finger: f64) -> bool {
        let vy = self.view_visible / self.view_total;
        let norm_handle = vy.max(self.min_handle_size / self.scroll_size);
        
        let new_scroll_pos = (
            (self.view_total * (1. - vy) * (finger / self.scroll_size)) / (1. - norm_handle)
        ).max(0.).min(self.view_total - self.view_visible);
        //log!("SCROLL POS {} {}", new_scroll_pos, self.view_total - self.view_visible);
        // lets snap new_scroll_pos
        let changed = self.scroll_pos != new_scroll_pos;
        self.scroll_pos = new_scroll_pos;
        self.scroll_target = new_scroll_pos;
        changed
    }
    
    // writes the norm_scroll value into the shader
    pub fn update_shader_scroll_pos(&mut self, cx: &mut Cx) {
        let (norm_scroll, _) = self.get_normalized_scroll_pos();
        self.draw_bar.apply_over(cx, live!{
            norm_scroll: (norm_scroll)
        });
        //self.draw_bg.set_norm_scroll(cx, norm_scroll);
    }
    
    // turns scroll_pos into an event on this.event
    pub fn make_scroll_action(&mut self) -> ScrollBarAction {
        ScrollBarAction::Scroll {
            scroll_pos: self.scroll_pos,
            view_total: self.view_total,
            view_visible: self.view_visible
        }
    }
    
    pub fn move_towards_scroll_target(&mut self, cx: &mut Cx) -> bool {
        if self.smoothing.is_none() {
            return false;
        }
        if (self.scroll_target - self.scroll_pos).abs() < 0.01 {
            return false
        }
        if self.scroll_pos > self.scroll_target { // go back
            self.scroll_pos = self.scroll_pos + (self.smoothing.unwrap() * self.scroll_delta).min(-1.);
            if self.scroll_pos <= self.scroll_target { // hit the target
                self.scroll_pos = self.scroll_target;
                self.update_shader_scroll_pos(cx);
                return false;
            }
        }
        else { // go forward
            self.scroll_pos = self.scroll_pos + (self.smoothing.unwrap() * self.scroll_delta).max(1.);
            if self.scroll_pos > self.scroll_target { // hit the target
                self.scroll_pos = self.scroll_target;
                self.update_shader_scroll_pos(cx);
                return false;
            }
        }
        self.update_shader_scroll_pos(cx);
        true
    }
    
    pub fn get_scroll_pos(&self) -> f64 {
        return self.scroll_pos;
    }
    
    pub fn set_scroll_pos_no_action(&mut self, cx: &mut Cx, scroll_pos: f64) -> bool {
        let scroll_pos = scroll_pos.min(self.view_total - self.view_visible).max(0.);
        if self.scroll_pos != scroll_pos {
            self.scroll_pos = scroll_pos;
            self.scroll_target = scroll_pos;
            self.update_shader_scroll_pos(cx);
            return true
        };
        return false
    }
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, scroll_pos: f64) -> bool {
        let scroll_pos = scroll_pos.min(self.view_total - self.view_visible).max(0.);
        if self.scroll_pos != scroll_pos {
            self.scroll_pos = scroll_pos;
            self.scroll_target = scroll_pos;
            self.update_shader_scroll_pos(cx);
            self.next_frame = cx.new_next_frame();
            return true
        };
        return false
    }
    
    
    pub fn set_scroll_pos_no_clip(&mut self, cx: &mut Cx, scroll_pos: f64) -> bool {
        if self.scroll_pos != scroll_pos {
            self.scroll_pos = scroll_pos;
            self.scroll_target = scroll_pos;
            self.update_shader_scroll_pos(cx);
            self.next_frame = cx.new_next_frame();
            return true
        };
        return false
    }
    
    pub fn get_scroll_target(&mut self) -> f64 {
        return self.scroll_target
    }
    
    pub fn set_scroll_view_total(&mut self, _cx: &mut Cx, view_total: f64) {
        self.view_total = view_total;
    }
    
    pub fn get_scroll_view_total(&self) -> f64 {
        return self.view_total;
    }
    
    pub fn get_scroll_view_visible(&self) -> f64 {
        return self.view_visible;
    }
    
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, scroll_pos_target: f64) -> bool {
        // clamp scroll_pos to
        
        let new_target = scroll_pos_target.min(self.view_total - self.view_visible).max(0.);
        if self.scroll_target != new_target {
            self.scroll_target = new_target;
            self.scroll_delta = new_target - self.scroll_pos;
            self.next_frame = cx.new_next_frame();
            return true
        };
        return false
    }
    
    pub fn scroll_into_view(&mut self, cx: &mut Cx, pos: f64, size: f64, smooth: bool) {
        if pos < self.scroll_pos { // scroll up
            let scroll_to = pos;
            if !smooth || self.smoothing.is_none() {
                self.set_scroll_pos(cx, scroll_to);
            }
            else {
                self.set_scroll_target(cx, scroll_to);
            }
        }
        else if pos + size > self.scroll_pos + self.view_visible { // scroll down
            let scroll_to = (pos + size) - self.view_visible;
            if pos + size > self.view_total { // resize _view_total if need be
                self.view_total = pos + size;
            }
            if !smooth || self.smoothing.is_none() {
                self.set_scroll_pos(cx, scroll_to);
            }
            else {
                self.set_scroll_target(cx, scroll_to);
            }
        }
    }
    
    pub fn handle_scroll_event(&mut self, cx: &mut Cx, event: &Event, scroll_area: Area, dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarAction)) {
        if let Event::Scroll(e) = event {
            if scroll_area.rect(cx).contains(e.abs) {
                if !match self.axis {
                    ScrollAxis::Horizontal => e.handled_x.get(),
                    ScrollAxis::Vertical => e.handled_y.get()
                } {
                    let scroll = match self.axis {
                        ScrollAxis::Horizontal => if self.use_vertical_finger_scroll {e.scroll.y}else {e.scroll.x},
                        ScrollAxis::Vertical => e.scroll.y
                    };
                    if !self.smoothing.is_none() && e.is_mouse {
                        let scroll_pos_target = self.get_scroll_target();
                        if self.set_scroll_target(cx, scroll_pos_target + scroll) {
                            match self.axis {
                                ScrollAxis::Horizontal => e.handled_x.set(true),
                                ScrollAxis::Vertical => e.handled_y.set(true)
                            }
                        };
                        self.move_towards_scroll_target(cx); // take the first step now
                        return dispatch_action(cx, self.make_scroll_action());
                    }
                    else {
                        let scroll_pos = self.get_scroll_pos();
                        if self.set_scroll_pos(cx, scroll_pos + scroll) {
                            match self.axis {
                                ScrollAxis::Horizontal => e.handled_x.set(true),
                                ScrollAxis::Vertical => e.handled_y.set(true)
                            }
                        }
                        return dispatch_action(cx, self.make_scroll_action());
                    }
                }
            }
        }
    }
    pub fn is_area_captured(&self, cx:&Cx)->bool{
        cx.fingers.is_area_captured(self.draw_bar.area())
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarAction)) {
        if self.visible {
            self.animator_handle_event(cx, event);
            if self.next_frame.is_event(event).is_some() {
                if self.move_towards_scroll_target(cx) {
                    self.next_frame = cx.new_next_frame();
                }
                return dispatch_action(cx, self.make_scroll_action());
            }
            
            match event.hits(cx, self.draw_bar.area()) {
                Hit::FingerDown(fe) => {
                    self.animator_play(cx, id!(hover.pressed));
                    let rel = fe.abs - fe.rect.pos;
                    let rel = match self.axis {
                        ScrollAxis::Horizontal => rel.x,
                        ScrollAxis::Vertical => rel.y
                    };
                    let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                    let bar_start = norm_scroll * self.scroll_size;
                    let bar_size = norm_handle * self.scroll_size;
                    if rel < bar_start || rel > bar_start + bar_size { // clicked outside
                        self.drag_point = Some(bar_size * 0.5);
                        if self.set_scroll_pos_from_finger(rel - self.drag_point.unwrap()){
                            dispatch_action(cx, self.make_scroll_action());
                        }
                    }
                    else { // clicked on
                        self.drag_point = Some(rel - bar_start); // store the drag delta
                    }
                },
                Hit::FingerHoverIn(_) => {
                    self.animator_play(cx, id!(hover.on));
                },
                Hit::FingerHoverOut(_) => {
                    self.animator_play(cx, id!(hover.off));
                },
                Hit::FingerUp(fe) => {
                    self.drag_point = None;
                    if fe.is_over && fe.device.has_hovers() {
                        self.animator_play(cx, id!(hover.on));
                    }
                    else {
                        self.animator_play(cx, id!(hover.off));
                    }
                    return;
                },
                Hit::FingerMove(fe) => {
                    let rel = fe.abs - fe.rect.pos;
                    // helper called by event code to scroll from a finger
                    if self.drag_point.is_none() {
                        // state should never occur.
                        //println!("Invalid state in scrollbar, fingerMove whilst drag_point is none")
                    }
                    else {
                        match self.axis {
                            ScrollAxis::Horizontal => {
                                if self.set_scroll_pos_from_finger(rel.x - self.drag_point.unwrap()){
                                    dispatch_action(cx, self.make_scroll_action());
                                }
                            },
                            ScrollAxis::Vertical => {
                                if self.set_scroll_pos_from_finger(rel.y - self.drag_point.unwrap()){
                                    dispatch_action(cx, self.make_scroll_action());
                                }
                            }
                        }
                    }
                 },
                _ => ()
            };
        }
    }
    
    pub fn draw_scroll_bar(&mut self, cx: &mut Cx2d, axis: ScrollAxis, view_rect: Rect, view_total: DVec2) -> f64 {
        
        self.axis = axis;
        
        match self.axis {
            ScrollAxis::Horizontal => {
                self.visible = view_total.x > view_rect.size.x + 0.1;
                self.scroll_size = if view_total.y > view_rect.size.y + 0.1 {
                    view_rect.size.x - self.bar_size
                }
                else {
                    view_rect.size.x
                } -self.bar_side_margin * 2.;
                self.view_total = view_total.x;
                self.view_visible = view_rect.size.x;
                self.scroll_pos = self.scroll_pos.min(self.view_total - self.view_visible).max(0.);
                
                if self.visible {
                    let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                    self.draw_bar.is_vertical = 0.0;
                    self.draw_bar.norm_scroll = norm_scroll as f32;
                    self.draw_bar.norm_handle = norm_handle as f32;
                    let scroll = cx.turtle().scroll();
                    self.draw_bar.draw_rel(
                        cx,
                        Rect {
                            pos: dvec2(self.bar_side_margin, view_rect.size.y - self.bar_size) + scroll,
                            size: dvec2(self.scroll_size, self.bar_size),
                        }
                    );
                }
            },
            ScrollAxis::Vertical => {
                // compute if we need a horizontal one
                self.visible = view_total.y > view_rect.size.y + 0.1;
                self.scroll_size = if view_total.x > view_rect.size.x + 0.1 {
                    view_rect.size.y - self.bar_size
                }
                else {
                    view_rect.size.y
                } -self.bar_side_margin * 2.;
                self.view_total = view_total.y;
                self.view_visible = view_rect.size.y;
                self.scroll_pos = self.scroll_pos.min(self.view_total - self.view_visible).max(0.);
                if self.visible {
                    let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                    self.draw_bar.is_vertical = 1.0;
                    self.draw_bar.norm_scroll = norm_scroll as f32;
                    self.draw_bar.norm_handle = norm_handle as f32;
                    let scroll = cx.turtle().scroll();
                    self.draw_bar.draw_rel(
                        cx,
                        Rect {
                            pos: dvec2(view_rect.size.x - self.bar_size, self.bar_side_margin) + scroll,
                            size: dvec2(self.bar_size, self.scroll_size)
                        }
                    );
                }
            }
        }
        
        
        // see if we need to clamp
        let clamped_pos = self.scroll_pos.min(self.view_total - self.view_visible).max(0.);
        if clamped_pos != self.scroll_pos {
            self.scroll_pos = clamped_pos;
            self.scroll_target = clamped_pos;
            // ok so this means we 'scrolled' this can give a problem for virtual viewport widgets
            self.next_frame = cx.new_next_frame();
        }
        
        self.scroll_pos
    }
}