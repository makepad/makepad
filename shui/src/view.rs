use crate::cx::*;

#[derive(Clone)]
pub enum ScrollBarsActive{
    None,
    Horizontal,
    Vertical,
    Both
}

#[derive(Clone)]
pub enum ScrollBarOrientation{
    Horizontal,
    Vertical,
}

impl Default for ScrollBarsActive{
    fn default()->Self{
        ScrollBarsActive::None
    }
}

pub trait ScrollBarLike<T>{
    fn new(cx:&mut Cx, orientation:ScrollBarOrientation)->T;
    fn draw_with_view_size(&mut self, cx:&mut Cx, view_area:Area, view_rect:Rect, total_size:Vec2);
    fn handle(&mut self, cx:&mut Cx, event:&mut Event)->ScrollBarEvent;
}

#[derive(Clone, PartialEq)]
pub enum ScrollBarEvent{
    None,
    ScrollHorizontal{scroll_pos:f32, view_total:f32, view_visible:f32},
    ScrollVertical{scroll_pos:f32, view_total:f32, view_visible:f32},
    ScrollDone
}

#[derive(Clone)]
pub struct View<T>
where T: ScrollBarLike<T> + Clone + ElementLife
{ // draw info per UI element
    pub draw_list_id:Option<usize>,
    pub clipped:bool,
    pub scroll_active: ScrollBarsActive,
    pub scroll_h:Element<T>,
    pub scroll_v:Element<T>,
}

impl<T> Style for View<T>
where T: ScrollBarLike<T> + Clone + ElementLife
{
    fn style(cx:&mut Cx)->Self{
        Self{
            clipped:true,
            draw_list_id:None,
            scroll_h:Element::<T>::new(T::new(cx, ScrollBarOrientation::Horizontal)),
            scroll_v:Element::<T>::new(T::new(cx, ScrollBarOrientation::Vertical)),
            scroll_active:ScrollBarsActive::None,
        }
    }
}

impl<T> View<T>
where T: ScrollBarLike<T> + Clone + ElementLife
{
    pub fn new(cx: &mut Cx,scroll_active:ScrollBarsActive)->Self{
        Self{
            scroll_active:scroll_active,
            ..Style::style(cx)
        }
    }

    pub fn begin(&mut self, cx:&mut Cx, layout:&Layout)->bool{
        // cx will have a path to a drawlist
        
        if self.draw_list_id.is_none(){ // draw node needs initialization
            if cx.draw_lists_free.len() != 0{
                self.draw_list_id = Some(cx.draw_lists_free.pop().unwrap());
            }
            else{
                self.draw_list_id =  Some(cx.draw_lists.len());
                cx.draw_lists.push(DrawList{..Default::default()});
            }
            let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
            draw_list.initialize(self.clipped);
        }
        else{
            // set len to 0
            let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
            draw_list.draw_calls_len = 0;
        }
        // push ourselves up the parent draw_stack
        if let Some(parent_draw_list_id) = cx.draw_list_stack.last(){

            // we need a new draw
            let parent_draw_list = &mut cx.draw_lists[*parent_draw_list_id];

            let id = parent_draw_list.draw_calls_len;
            parent_draw_list.draw_calls_len = parent_draw_list.draw_calls_len + 1;
            
            // see if we need to add a new one
            if parent_draw_list.draw_calls_len > parent_draw_list.draw_calls.len(){
                parent_draw_list.draw_calls.push({
                    DrawCall{
                        draw_call_id:parent_draw_list.draw_calls.len(),
                        sub_list_id:self.draw_list_id.unwrap(),
                        ..Default::default()
                    }
                })
            }
            else{// or reuse a sub list node
                let draw = &mut parent_draw_list.draw_calls[id];
                draw.sub_list_id = self.draw_list_id.unwrap();
            }
        }

        cx.current_draw_list_id = self.draw_list_id.unwrap();
        cx.draw_list_stack.push(self.draw_list_id.unwrap());
        
        cx.begin_turtle(layout);

        false
        //cx.turtle.x = 0.0;
        //cx.turtle.y = 0.0;
    }

    pub fn handle_scroll(&mut self, cx:&mut Cx, event:&mut Event)->ScrollBarEvent{
        let mut ret_h = ScrollBarEvent::None;
        let mut ret_v = ScrollBarEvent::None;
        for scroll_h in self.scroll_h.all(){
            ret_h = scroll_h.handle(cx, event);
        }
        for scroll_v in self.scroll_v.all(){
            ret_v = scroll_v.handle(cx, event);
        }
        match ret_h{
            ScrollBarEvent::None=>(),
            ScrollBarEvent::ScrollHorizontal{scroll_pos,..}=>{
                let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
                draw_list.set_scroll_x(scroll_pos);
            },
            _=>{return ret_h;}
        };
        match ret_v{
            ScrollBarEvent::None=>(),
            ScrollBarEvent::ScrollVertical{scroll_pos,..}=>{
                let draw_list = &mut cx.draw_lists[self.draw_list_id.unwrap()];
                draw_list.set_scroll_y(scroll_pos);
            },
            _=>{return ret_v;}
        };
        ScrollBarEvent::None
    }

    pub fn end(&mut self, cx:&mut Cx)->Area{
        // do scrollbars if need be
        cx.draw_list_stack.pop();

        let draw_list_id = self.draw_list_id.unwrap();
        let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id});

        // lets ask the turtle our actual bounds
        let view_total = cx.turtle_bounds();   
        let rect_now =  cx.turtle_rect();

        self.scroll_h.get(cx).draw_with_view_size(cx, view_area, rect_now, view_total);
        self.scroll_v.get(cx).draw_with_view_size(cx, view_area, rect_now, view_total);

        let rect = cx.end_turtle();
        let draw_list = &mut cx.draw_lists[draw_list_id];
        draw_list.rect = rect;
        return view_area
    }
}