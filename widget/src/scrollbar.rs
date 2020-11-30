use makepad_render::*;

#[derive(Clone)]
pub struct ScrollBar {
    
    pub bg: DrawScrollBar,
    pub bar_size: f32,
    pub min_handle_size: f32, //minimum size of the handle in pixels
    pub axis: Axis,
    pub animator: Animator,
    pub use_vertical_finger_scroll: bool,
    pub smoothing: Option<f32>,
    
    next_frame: NextFrame,
    visible: bool,
    bar_side_margin: f32,
    view_area: Area,
    view_total: f32, // the total view area
    view_visible: f32, // the visible view area
    scroll_size: f32, // the size of the scrollbar
    scroll_pos: f32, // scrolling position non normalised
    
    scroll_target: f32,
    scroll_delta: f32,
    
    drag_point: Option<f32>, // the point in pixels where we are dragging
}

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawScrollBar {
    #[default_shader(self::shader_bg)]
    base: DrawColor,
    is_vertical: f32,
    norm_handle: f32,
    norm_scroll: f32
}

#[derive(Clone, PartialEq, Debug)]
pub enum ScrollBarEvent {
    None,
    Scroll {scroll_pos: f32, view_total: f32, view_visible: f32},
    ScrollDone
}

impl ScrollBar {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            bar_size: 12.0,
            min_handle_size: 30.0,
            smoothing: None,
            
            next_frame: NextFrame::default(),
            axis: Axis::Horizontal,
            animator: Animator::default(),
            bg: DrawScrollBar::new(cx, default_shader!())
                .with_draw_depth(2.5),
            use_vertical_finger_scroll: false,
            
            visible: false,
            
            view_area: Area::Empty,
            view_total: 0.0,
            view_visible: 0.0,
            bar_side_margin: 6.0,
            scroll_size: 0.0,
            scroll_pos: 0.0,
            
            scroll_target: 0.0,
            scroll_delta: 0.0,
            
            drag_point: None,
        }
    }
    
    pub fn style(cx: &mut Cx) {
        self::DrawScrollBar::register_draw_input(cx);
        
        live_body!(cx, r#"
            self::color_base: #5;
            self::color_over: #7;
            self::color_down: #9;
            
            self::anim_default: Anim {
                play: Cut {duration: 0.5}
                tracks: [
                    Vec4 {keys: {1.0: self::color_base}, bind_to: makepad_render::drawcolor::DrawColor::color}
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.05}
                tracks: [
                    Vec4 {keys: {1.0: self::color_over}, bind_to: makepad_render::drawcolor::DrawColor::color}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.05}
                tracks: [
                    Vec4 {keys: {1.0: self::color_down}, bind_to: makepad_render::drawcolor::DrawColor::color}
                ]
            }
            
            self::shader_bg: Shader {
                use makepad_render::drawcolor::shader::*;
                
                draw_input: self::DrawScrollBar;
                
                const border_radius: float = 1.5;
                
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * rect_size);
                    if is_vertical > 0.5 {
                        df.box(1., rect_size.y * norm_scroll, rect_size.x * 0.5, rect_size.y * norm_handle, border_radius);
                    }
                    else {
                        df.box(rect_size.x * norm_scroll, 1., rect_size.x * norm_handle, rect_size.y * 0.5, border_radius);
                    }
                    return df.fill_keep(color);
                }
            }
        "#);
    }
    
    pub fn with_bar_size(self, bar_size: f32) -> Self {Self {bar_size, ..self}}
    pub fn with_smoothing(self, s: f32) -> Self {Self {smoothing: Some(s), ..self}}
    pub fn with_use_vertical_finger_scroll(self, use_vertical_finger_scroll: bool) -> Self {Self {use_vertical_finger_scroll, ..self}}
    
    // reads back normalized scroll position info
    pub fn get_normalized_scroll_pos(&self) -> (f32, f32) {
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
    pub fn set_scroll_pos_from_finger(&mut self, cx: &mut Cx, finger: f32) -> ScrollBarEvent {
        let vy = self.view_visible / self.view_total;
        let norm_handle = vy.max(self.min_handle_size / self.scroll_size);
        
        let new_scroll_pos = (
            (self.view_total * (1. - vy) * (finger / self.scroll_size)) / (1. - norm_handle)
        ).max(0.).min(self.view_total - self.view_visible);
        
        // lets snap new_scroll_pos
        let changed = self.scroll_pos != new_scroll_pos;
        self.scroll_pos = new_scroll_pos;
        self.scroll_target = new_scroll_pos;
        if changed {
            self.update_shader_scroll_pos(cx);
            return self.make_scroll_event();
        }
        return ScrollBarEvent::None;
    }
    
    // writes the norm_scroll value into the shader
    pub fn update_shader_scroll_pos(&mut self, cx: &mut Cx) {
        let (norm_scroll, _) = self.get_normalized_scroll_pos();
        self.bg.set_norm_scroll(cx, norm_scroll);
    }
    
    // turns scroll_pos into an event on this.event
    pub fn make_scroll_event(&mut self) -> ScrollBarEvent {
        ScrollBarEvent::Scroll {
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
    
    pub fn get_scroll_pos(&self) -> f32 {
        return self.scroll_pos;
    }
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, scroll_pos: f32) -> bool {
        // clamp scroll_pos to
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
    
    pub fn get_scroll_target(&mut self) -> f32 {
        return self.scroll_target
    }
    
    pub fn set_scroll_view_total(&mut self, _cx: &mut Cx, view_total: f32) {
        self.view_total = view_total;
    }
    
    pub fn get_scroll_view_total(&self) -> f32 {
        return self.view_total;
    }
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, scroll_pos_target: f32) -> bool {
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
    
    pub fn scroll_into_view(&mut self, cx: &mut Cx, pos: f32, size: f32, smooth: bool) {
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
    
    pub fn handle_scroll_bar(&mut self, cx: &mut Cx, event: &mut Event) -> ScrollBarEvent {
        // lets check if our view-area gets a mouse-scroll.
        match event {
            Event::FingerScroll(fe) => { //if !fe.handled {
                if !match self.axis {
                    Axis::Horizontal => fe.handled_x,
                    Axis::Vertical => fe.handled_y
                } {
                    let rect = self.view_area.get_rect(cx);
                    if rect.contains(fe.abs) { // handle mousewheel
                        // we should scroll in either x or y
                        let scroll = match self.axis {
                            Axis::Horizontal => if self.use_vertical_finger_scroll {fe.scroll.y}else {fe.scroll.x},
                            Axis::Vertical => fe.scroll.y
                        };
                        if !self.smoothing.is_none() && fe.input_type.is_mouse() {
                            let scroll_pos_target = self.get_scroll_target();
                            if self.set_scroll_target(cx, scroll_pos_target + scroll) {
                                match self.axis {
                                    Axis::Horizontal => fe.handled_x = true,
                                    Axis::Vertical => fe.handled_y = true
                                }
                            };
                            self.move_towards_scroll_target(cx); // take the first step now
                            return self.make_scroll_event();
                        }
                        else {
                            let scroll_pos = self.get_scroll_pos();
                            if self.set_scroll_pos(cx, scroll_pos + scroll) {
                                match self.axis {
                                    Axis::Horizontal => fe.handled_x = true,
                                    Axis::Vertical => fe.handled_y = true
                                }
                            }
                            return self.make_scroll_event();
                        }
                    }
                }
            },
            
            _ => ()
        };
        if self.visible {
            if let Some(ae) = event.is_animate(cx, &self.animator) {
                self.bg.animate(cx, &mut self.animator, ae.time);
            }
            if let Some(_) = event.is_next_frame(cx, self.next_frame) {
                if self.move_towards_scroll_target(cx) {
                    self.next_frame = cx.new_next_frame();
                }
                return self.make_scroll_event()
            }
            
            match event.hits(cx, self.bg.area(), HitOpt::default()) {
                Event::FingerDown(fe) => {
                    self.animator.play_anim(cx, live_anim!(cx, self::anim_down));
                    let rel = match self.axis {
                        Axis::Horizontal => fe.rel.x,
                        Axis::Vertical => fe.rel.y
                    };
                    cx.set_down_mouse_cursor(MouseCursor::Default);
                    let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                    let bar_start = norm_scroll * self.scroll_size;
                    let bar_size = norm_handle * self.scroll_size;
                    if rel < bar_start || rel > bar_start + bar_size { // clicked outside
                        self.drag_point = Some(bar_size * 0.5);
                        return self.set_scroll_pos_from_finger(cx, rel - self.drag_point.unwrap());
                    }
                    else { // clicked on
                        self.drag_point = Some(rel - bar_start); // store the drag delta
                    }
                },
                Event::FingerHover(fe) => {
                    if self.drag_point.is_none() {
                        cx.set_hover_mouse_cursor(MouseCursor::Default);
                        match fe.hover_state {
                            HoverState::In => {
                                self.animator.play_anim(cx, live_anim!(cx, self::anim_over));
                            },
                            HoverState::Out => {
                                self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                            },
                            _ => ()
                        }
                    }
                },
                Event::FingerUp(fe) => {
                    self.drag_point = None;
                    if fe.is_over {
                        if fe.input_type.has_hovers() {
                            self.animator.play_anim(cx, live_anim!(cx, self::anim_over));
                        }
                        else {
                            self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                        }
                    }
                    else {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                    }
                    return ScrollBarEvent::ScrollDone;
                },
                Event::FingerMove(fe) => {
                    // helper called by event code to scroll from a finger
                    if self.drag_point.is_none() {
                        // state should never occur.
                        //println!("Invalid state in scrollbar, fingerMove whilst drag_point is none")
                    }
                    else {
                        match self.axis {
                            Axis::Horizontal => {
                                return self.set_scroll_pos_from_finger(cx, fe.rel.x - self.drag_point.unwrap());
                            },
                            Axis::Vertical => {
                                return self.set_scroll_pos_from_finger(cx, fe.rel.y - self.drag_point.unwrap());
                            }
                        }
                    }
                },
                _ => ()
            };
        }
        
        ScrollBarEvent::None
    }
    
    pub fn draw_scroll_bar(&mut self, cx: &mut Cx, axis: Axis, view_area: Area, view_rect: Rect, view_total: Vec2) -> f32 {

        if self.animator.need_init(cx) {
            self.animator.init(cx, live_anim!(cx, self::anim_default));
            self.bg.last_animate(&self.animator);
        }
        self.view_area = view_area;
        self.axis = axis;
        
        match self.axis {
            Axis::Horizontal => {
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
                    self.bg.is_vertical = 0.0;
                    self.bg.norm_scroll = norm_scroll;
                    self.bg.norm_handle = norm_handle;
                    self.bg.draw_quad_rel(
                        cx,
                        Rect {
                            pos: vec2(self.bar_side_margin, view_rect.size.y - self.bar_size),
                            size: vec2( self.scroll_size, self.bar_size),
                        }
                    );
                    self.bg.area().set_do_scroll(cx, false, false);
                }
            },
            Axis::Vertical => {
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
                    self.bg.is_vertical = 1.0;
                    self.bg.norm_scroll = norm_scroll;
                    self.bg.norm_handle = norm_handle;
                    self.bg.draw_quad_rel(
                        cx,
                        Rect {
                            pos: vec2(view_rect.size.x - self.bar_size, self.bar_side_margin),
                            size: vec2(self.bar_size, self.scroll_size)
                        }
                    );
                    self.bg.area().set_do_scroll(cx, false, false);
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
