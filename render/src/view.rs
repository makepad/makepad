use crate::cx::*;

pub trait ScrollBarLike<ScrollBarT> {
    fn draw_scroll_bar(&mut self, cx: &mut Cx, axis: Axis, view_area: Area, view_rect: Rect, total_size: Vec2) -> f32;
    fn handle_scroll_bar(&mut self, cx: &mut Cx, event: &mut Event) -> ScrollBarEvent;
    fn set_scroll_view_total(&mut self, cx: &mut Cx, view_total: f32);
    fn get_scroll_view_total(&self) -> f32;
    fn set_scroll_pos(&mut self, cx: &mut Cx, scroll_pos: f32) -> bool;
    fn get_scroll_pos(&self) -> f32;
    fn scroll_into_view(&mut self, cx: &mut Cx, pos: f32, size: f32);
    fn get_scroll_target(&mut self) -> f32;
    fn set_scroll_target(&mut self, cx: &mut Cx, scroll_pos_target: f32) -> bool;
}

#[derive(Clone, PartialEq, Debug)]
pub enum ScrollBarEvent {
    None,
    Scroll {scroll_pos: f32, view_total: f32, view_visible: f32},
    ScrollDone
}

#[derive(Clone)]
pub struct View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone
{ // draw info per UI element
    pub draw_list_id: Option<usize>,
    pub is_clipped: bool,
    pub is_overlay: bool, // this view is an overlay, rendered last
    pub scroll_h: Option<TScrollBar>,
    pub scroll_v: Option<TScrollBar>,
}

impl<TScrollBar> Style for View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone
{
    fn style(_cx: &mut Cx) -> Self {
        Self {
            is_clipped: true,
            is_overlay: false,
            draw_list_id: None,
            scroll_h: None,
            scroll_v: None
        }
    }
}

#[derive(Clone)]
pub struct NoScrollBar {
}

impl NoScrollBar {
    fn handle_no_scroll_bar(&mut self, _cx: &mut Cx, _event: &mut Event) -> ScrollBarEvent {
        ScrollBarEvent::None
    }
}

impl ScrollBarLike<NoScrollBar> for NoScrollBar {
    fn draw_scroll_bar(&mut self, _cx: &mut Cx, _axis: Axis, _view_area: Area, _view_rect: Rect, _total_size: Vec2) -> f32 {0.}
    fn handle_scroll_bar(&mut self, _cx: &mut Cx, _event: &mut Event) -> ScrollBarEvent {
        ScrollBarEvent::None
    }
    fn set_scroll_view_total(&mut self, _cx: &mut Cx, _view_total: f32) {}
    fn get_scroll_view_total(&self) -> f32 {0.}
    fn set_scroll_pos(&mut self, _cx: &mut Cx, _scroll_pos: f32) -> bool {false}
    fn get_scroll_pos(&self) -> f32 {0.}
    fn scroll_into_view(&mut self, _cx: &mut Cx, _pos: f32, _size: f32) {}
    fn set_scroll_target(&mut self, _cx: &mut Cx, _scroll_target: f32) -> bool {false}
    fn get_scroll_target(&mut self) -> f32 {0.}
}

pub type ViewRedraw = Result<(), ()>;

impl<TScrollBar> View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone
{
    
    pub fn begin_view(&mut self, cx: &mut Cx, layout: &Layout) -> ViewRedraw {
        
        if !cx.is_in_redraw_cycle {
            panic!("calling begin_view outside of redraw cycle is not possible!");
        }
        
        if self.draw_list_id.is_none() { // we need a draw_list_id
            if cx.draw_lists_free.len() != 0 {
                self.draw_list_id = Some(cx.draw_lists_free.pop().unwrap());
            }
            else {
                self.draw_list_id = Some(cx.draw_lists.len());
                cx.draw_lists.push(DrawList {..Default::default()});
            }
            let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
            draw_list.initialize(self.is_clipped, cx.redraw_id);
        }
        
        let draw_list_id = self.draw_list_id.unwrap();
        
        // check if we have a view parent
        let window_id = *cx.window_stack.last().expect("No window found when begin_view");
        
        let nesting_draw_list_id = if cx.draw_list_stack.len() > 0 {
            *cx.draw_list_stack.last().unwrap()
        }
        else { // return the root draw list
            0
        };
        
        let override_layout = if cx.windows[window_id].root_draw_list_id.is_none() {
            // we are the first view on a window
            let window = &mut cx.windows[window_id];
            window.root_draw_list_id = Some(draw_list_id);
            // we should take the window geometry and abs position as our turtle layout
            Layout {
                abs_origin: Some(Vec2 {x: 0., y: 0.}),
                abs_size: Some(window.get_inner_size()),
                ..layout.clone()
            }
        }
        else {
            layout.clone()
        };
        
        let window = &mut cx.windows[window_id];
        
        // find the parent draw list id
        let parent_draw_list_id = if self.is_overlay {
            if window.root_draw_list_id.is_none() {
                panic!("Cannot make overlay inside window without root view")
            };
            let root_draw_list_id = window.root_draw_list_id.unwrap();
            root_draw_list_id
        }
        else {
            if let Some(last_draw_list) = cx.draw_list_stack.last() {
                *last_draw_list
            }
            else { // we have no parent
                draw_list_id
            }
        };
        
        // push ourselves up the parent draw_stack
        if draw_list_id != parent_draw_list_id {
            
            // we need a new draw
            let parent_draw_list = &mut cx.draw_lists[parent_draw_list_id];
            
            let id = parent_draw_list.draw_calls_len;
            parent_draw_list.draw_calls_len = parent_draw_list.draw_calls_len + 1;
            
            // see if we need to add a new one
            if parent_draw_list.draw_calls_len > parent_draw_list.draw_calls.len() {
                parent_draw_list.draw_calls.push({
                    DrawCall {
                        draw_list_id: parent_draw_list_id,
                        draw_call_id: parent_draw_list.draw_calls.len(),
                        redraw_id: cx.redraw_id,
                        sub_list_id: draw_list_id,
                        ..Default::default()
                    }
                })
            }
            else { // or reuse a sub list node
                let draw = &mut parent_draw_list.draw_calls[id];
                draw.sub_list_id = draw_list_id;
                draw.redraw_id = cx.redraw_id;
            }
        }
        
        // set nesting draw list id for incremental repaint scanning
        cx.draw_lists[draw_list_id].nesting_draw_list_id = nesting_draw_list_id;
        
        if cx.draw_lists[draw_list_id].draw_calls_len != 0 && !cx.draw_list_needs_redraw(draw_list_id) {
            // walk the turtle because we aren't drawing
            let w = Bounds::Fix(cx.draw_lists[draw_list_id].rect.w);
            let h = Bounds::Fix(cx.draw_lists[draw_list_id].rect.h);
            cx.walk_turtle(w, h, layout.margin, None);
            return Err(());
        }
        
        // prepare drawlist for drawing
        let draw_list = &mut cx.draw_lists[draw_list_id];
        
        // update drawlist ids
        draw_list.redraw_id = cx.redraw_id;
        draw_list.draw_calls_len = 0;
        
        cx.draw_list_stack.push(draw_list_id);
        
        cx.begin_turtle(&override_layout, Area::DrawList(DrawListArea {
            draw_list_id: draw_list_id,
            redraw_id: cx.redraw_id
        }));
        
        Ok(())
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
                let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
                draw_list.set_scroll_x(scroll_pos);
                cx.paint_dirty = true;
            },
            _ => ()
        };
        match ret_v {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
                draw_list.set_scroll_y(scroll_pos);
                cx.paint_dirty = true;
            },
            _ => ()
        };
        ret_h != ScrollBarEvent::None || ret_v != ScrollBarEvent::None
    }
    
    pub fn get_scroll_pos(&self, cx: &Cx) -> Vec2 {
        if let Some(draw_list_id) = self.draw_list_id {
            let draw_list = &cx.draw_lists[draw_list_id];
            draw_list.get_scroll_pos()
        }
        else {
            Vec2::zero()
        }
    }
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: Vec2) -> bool {
        let draw_list_id = self.draw_list_id.unwrap();
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if let Some(scroll_h) = &mut self.scroll_h {
            if scroll_h.set_scroll_pos(cx, pos.x) {
                let scroll_pos = scroll_h.get_scroll_pos();
                cx.draw_lists[draw_list_id].set_scroll_x(scroll_pos);
                changed = true;
            }
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            if scroll_v.set_scroll_pos(cx, pos.y) {
                let scroll_pos = scroll_v.get_scroll_pos();
                cx.draw_lists[draw_list_id].set_scroll_y(scroll_pos);
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
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, pos: Vec2) {
        if let Some(scroll_h) = &mut self.scroll_h {
            scroll_h.set_scroll_target(cx, pos.x);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            scroll_v.set_scroll_target(cx, pos.y);
        }
    }
    
    pub fn end_view(&mut self, cx: &mut Cx) -> Area {
        let draw_list_id = self.draw_list_id.unwrap();
        let view_area = Area::DrawList(DrawListArea {draw_list_id: draw_list_id, redraw_id: cx.redraw_id});
        
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
            // so, in a virtual viewport widget this isn't actually allowed
            
            cx.draw_lists[draw_list_id].set_scroll_x(scroll_pos);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            //println!("SET SCROLLBAR {} {}", rect_now.h, view_total.y);
            let scroll_pos = scroll_v.draw_scroll_bar(cx, Axis::Vertical, view_area, rect_now, view_total);
            cx.draw_lists[draw_list_id].set_scroll_y(scroll_pos);
        }
        
        let rect = cx.end_turtle(view_area);
        
        let draw_list = &mut cx.draw_lists[draw_list_id];
        
        draw_list.rect = rect;
        
        cx.draw_list_stack.pop();
        
        return view_area
    }
    
    pub fn get_rect(&mut self, cx: &Cx) -> Rect {
        if let Some(draw_list_id) = self.draw_list_id {
            let draw_list = &cx.draw_lists[draw_list_id];
            return draw_list.rect
        }
        Rect::zero()
    }
    
    
    pub fn redraw_view_area(&self, cx: &mut Cx) {
        if let Some(draw_list_id) = self.draw_list_id {
            let draw_list = &cx.draw_lists[draw_list_id];
            let area = Area::DrawList(DrawListArea {draw_list_id: draw_list_id, redraw_id: draw_list.redraw_id});
            cx.redraw_area(area);
        }
        else {
            cx.redraw_area(Area::All)
        }
    }
    
    pub fn get_view_area(&self, cx: &Cx) -> Area {
        if let Some(draw_list_id) = self.draw_list_id {
            let draw_list = &cx.draw_lists[draw_list_id];
            Area::DrawList(DrawListArea {draw_list_id: draw_list_id, redraw_id: draw_list.redraw_id})
        }
        else {
            Area::Empty
        }
    }
}