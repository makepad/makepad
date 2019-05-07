use crate::cx::*;

pub trait ScrollBarLike<ScrollBarT>{
    fn draw_scroll_bar(&mut self, cx:&mut Cx, axis:Axis, view_area:Area, view_rect:Rect, total_size:Vec2)->f32;
    fn handle_scroll_bar(&mut self, cx:&mut Cx, event:&mut Event)->ScrollBarEvent;
    fn set_scroll_pos(&mut self, cx:&mut Cx, scroll_pos:f32)->bool;
    fn get_scroll_pos(&self)->f32;
    fn scroll_into_view(&mut self, cx:&mut Cx, pos:f32, size:f32);
}

#[derive(Clone, PartialEq)]
pub enum ScrollBarEvent{
    None,
    Scroll{scroll_pos:f32, view_total:f32, view_visible:f32},
    ScrollDone
}

#[derive(Clone)]
pub struct View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone + ElementLife
{ // draw info per UI element
    pub draw_list_id:Option<usize>,
    pub is_clipped:bool,
    pub is_overlay:bool,// this view is an overlay, rendered last
    pub scroll_h:Option<TScrollBar>,
    pub scroll_v:Option<TScrollBar>,
}

impl<TScrollBar> Style for View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone + ElementLife
{
    fn style(_cx:&mut Cx)->Self{
        Self{
            is_clipped:true,
            is_overlay:false,
            draw_list_id:None,
            scroll_h:None,
            scroll_v:None
        }
    }
}

#[derive(Clone)]
pub struct NoScrollBar{
}

impl NoScrollBar{
    fn handle_no_scroll_bar(&mut self, _cx:&mut Cx, _event:&mut Event)->ScrollBarEvent{
        ScrollBarEvent::None
    }
}

impl ElementLife for NoScrollBar{
    fn construct(&mut self, _cx:&mut Cx){}
    fn destruct(&mut self, _cx:&mut Cx){}
}

impl ScrollBarLike<NoScrollBar> for NoScrollBar{
    fn draw_scroll_bar(&mut self, _cx:&mut Cx, _axis:Axis, _view_area:Area, _view_rect:Rect, _total_size:Vec2)->f32{0.}
    fn handle_scroll_bar(&mut self, _cx:&mut Cx, _event:&mut Event)->ScrollBarEvent{
        ScrollBarEvent::None
    }
    fn set_scroll_pos(&mut self, _cx:&mut Cx, _scroll_pos:f32)->bool{false}
    fn get_scroll_pos(&self)->f32{0.}
    fn scroll_into_view(&mut self, _cx:&mut Cx, _pos:f32, _size:f32){}
}

impl<TScrollBar> View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone + ElementLife
{


    pub fn begin_view(&mut self, cx:&mut Cx, layout:&Layout)->bool{
        // do a dirty check
        if !cx.is_in_redraw_cycle{
            panic!("calling begin_view outside of redraw cycle is not possible!");
        }
        
        if self.draw_list_id.is_none(){ // draw node needs initialization
            if cx.draw_lists_free.len() != 0{
                self.draw_list_id = Some(cx.draw_lists_free.pop().unwrap());
            }
            else{
                self.draw_list_id =  Some(cx.draw_lists.len());
                cx.draw_lists.push(DrawList{..Default::default()});
            }
            let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
            draw_list.initialize(self.is_clipped, cx.redraw_id);
        }
        else{
            // set len to 0
            let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
            draw_list.redraw_id = cx.redraw_id;
            draw_list.draw_calls_len = 0;
        }
        let draw_list_id = self.draw_list_id.unwrap();
        
        let nesting_draw_list_id = cx.current_draw_list_id;
       
        let parent_draw_list_id = if self.is_overlay{
            0
        }
        else {
            nesting_draw_list_id
        };

        // push ourselves up the parent draw_stack
        if draw_list_id != parent_draw_list_id{
            
            // we need a new draw
            let parent_draw_list = &mut cx.draw_lists[parent_draw_list_id];

            let id = parent_draw_list.draw_calls_len;
            parent_draw_list.draw_calls_len = parent_draw_list.draw_calls_len + 1;
            
            // see if we need to add a new one
            if parent_draw_list.draw_calls_len > parent_draw_list.draw_calls.len(){
                parent_draw_list.draw_calls.push({
                    DrawCall{
                        draw_list_id:parent_draw_list_id,
                        draw_call_id:parent_draw_list.draw_calls.len(),
                        redraw_id:cx.redraw_id,
                        sub_list_id:draw_list_id,
                        ..Default::default()
                    }
                })
            }
            else{// or reuse a sub list node
                let draw = &mut parent_draw_list.draw_calls[id];
                draw.sub_list_id = draw_list_id;
                draw.redraw_id = cx.redraw_id;
            }
        }
 
        // set nesting draw list id for incremental repaint scanning
        let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
        draw_list.nesting_draw_list_id = nesting_draw_list_id;
        cx.draw_list_stack.push(cx.current_draw_list_id);
        cx.current_draw_list_id = draw_list_id;

        cx.begin_turtle(layout, Area::DrawList(DrawListArea{
            draw_list_id:draw_list_id,
            redraw_id:cx.redraw_id
        }));

        false
        //cx.turtle.x = 0.0;
        //cx.turtle.y = 0.0;
    }

    pub fn handle_scroll_bars(&mut self, cx:&mut Cx, event:&mut Event)->(ScrollBarEvent,ScrollBarEvent){
        let mut ret_h = ScrollBarEvent::None;
        let mut ret_v = ScrollBarEvent::None;

        if let Some(scroll_h) = &mut self.scroll_h{
            //if let Some(scroll_h) = &mut scroll_h.element{
            ret_h = scroll_h.handle_scroll_bar(cx, event);
            //}
        }
        if let Some(scroll_v) = &mut self.scroll_v{
            //if let Some(scroll_v) = &mut scroll_v.element{
            ret_v = scroll_v.handle_scroll_bar(cx, event);
            //}
        }
        match ret_h{
            ScrollBarEvent::None=>(),
            ScrollBarEvent::Scroll{scroll_pos,..}=>{
                let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
                draw_list.set_scroll_x(scroll_pos);
            },
            _=>()
        };
        match ret_v{
            ScrollBarEvent::None=>(),
            ScrollBarEvent::Scroll{scroll_pos,..}=>{
                let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
                draw_list.set_scroll_y(scroll_pos);
            },
            _=>()
        };
        (ret_h, ret_v)
    }

    pub fn get_scroll_pos(&self, cx:&Cx)->Vec2{
        if let Some(draw_list_id) = self.draw_list_id{
            let draw_list = &cx.draw_lists[draw_list_id];
            draw_list.get_scroll_pos()
        }
        else{
            Vec2::zero()
        }
   }

    pub fn set_scroll_pos(&mut self, cx:&mut Cx, pos:Vec2)->bool{
        let draw_list_id = self.draw_list_id.unwrap();
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if let Some(scroll_h) = &mut self.scroll_h{
            if scroll_h.set_scroll_pos(cx, pos.x){
                let scroll_pos = scroll_h.get_scroll_pos();
                cx.draw_lists[draw_list_id].set_scroll_x(scroll_pos);
                changed = true;
            }
        }
        if let Some(scroll_v) = &mut self.scroll_v{
            if scroll_v.set_scroll_pos(cx, pos.y){
                let scroll_pos = scroll_v.get_scroll_pos();
                cx.draw_lists[draw_list_id].set_scroll_y(scroll_pos);
                changed = true;
            }
        }
        changed
    }

    pub fn scroll_into_view(&mut self, cx:&mut Cx, rect:Rect){
        if let Some(scroll_h) = &mut self.scroll_h{
            scroll_h.scroll_into_view(cx, rect.x, rect.w);
        }
        if let Some(scroll_v) = &mut self.scroll_v{
            scroll_v.scroll_into_view(cx, rect.y, rect.h);
        }
    }

    pub fn end_view(&mut self, cx:&mut Cx)->Area{
        let draw_list_id = self.draw_list_id.unwrap();
        let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});

        // lets ask the turtle our actual bounds
        let view_total = cx.turtle_bounds();   
        let mut rect_now =  cx.turtle_rect();
        if rect_now.h.is_nan(){
            rect_now.h = view_total.y;
        }
        if rect_now.w.is_nan(){
            rect_now.w = view_total.x;
        }

        if let Some(scroll_h) = &mut self.scroll_h{
            let scroll_pos = scroll_h.draw_scroll_bar(cx, Axis::Horizontal, view_area, rect_now, view_total);
            // so, in a virtual viewport widget this isn't actually allowed
            
            cx.draw_lists[draw_list_id].set_scroll_x(scroll_pos);
        }
        if let Some(scroll_v) = &mut self.scroll_v{
            let scroll_pos = scroll_v.draw_scroll_bar(cx, Axis::Vertical,view_area, rect_now, view_total);
            cx.draw_lists[draw_list_id].set_scroll_y(scroll_pos);
        }
        
        let rect = cx.end_turtle(view_area);

        let draw_list = &mut cx.draw_lists[draw_list_id];

        draw_list.rect = rect;

        // only now pop the drawlist
        cx.current_draw_list_id = cx.draw_list_stack.pop().unwrap();
        
       return view_area
    }

    pub fn get_rect(&mut self, cx:&Cx)->Rect{
        if let Some(draw_list_id) = self.draw_list_id{
            let draw_list = &cx.draw_lists[draw_list_id];
            return draw_list.rect
        }
        Rect::zero()
    }


    pub fn redraw_view_area(&self, cx:&mut Cx){
        if let Some(draw_list_id) = self.draw_list_id{
            let draw_list = &cx.draw_lists[draw_list_id];
            let area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:draw_list.redraw_id});
            cx.redraw_area(area);
        }
        else{
            cx.redraw_area(Area::All)
        }
    }

    pub fn get_view_area(&self, cx:&Cx)->Area{
        if let Some(draw_list_id) = self.draw_list_id{
            let draw_list = &cx.draw_lists[draw_list_id];
            Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:draw_list.redraw_id})
        }
        else{
            Area::Empty
        }
    }
}