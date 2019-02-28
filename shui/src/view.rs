use crate::cx_shared::*;
use crate::cxturtle::*;
use crate::cxdrawing_shared::*;

#[derive(Default,Clone)]
pub struct View{ // draw info per UI element
    pub id:usize,
    pub frame_id:usize,
    pub initialized:bool,
    // the set of shader_id + 
    pub draw_list_id:usize
}

impl View{
    pub fn new()->Self{
        Self{
            ..Default::default()
        }
    }

    pub fn begin(&mut self, cx:&mut Cx, layout:&Layout)->bool{

        // cx will have a path to a drawlist

        if !self.initialized{ // draw node needs initialization
            if cx.drawing.draw_lists_free.len() != 0{
                self.draw_list_id = cx.drawing.draw_lists_free.pop().unwrap();
            }
            else{
                self.draw_list_id = cx.drawing.draw_lists.len();
                cx.drawing.draw_lists.push(DrawList{..Default::default()});
            }
            self.initialized = true;
            let draw_list = &mut cx.drawing.draw_lists[self.draw_list_id];
            draw_list.initialize();
        }
        else{
            // set len to 0
            let draw_list = &mut cx.drawing.draw_lists[self.draw_list_id];
            draw_list.draw_calls_len = 0;
        }
        // push ourselves up the parent draw_stack
        if let Some(parent_view) = cx.drawing.view_stack.last(){

            // we need a new draw
            let parent_draw_list = &mut cx.drawing.draw_lists[parent_view.draw_list_id];

            let id = parent_draw_list.draw_calls_len;
            parent_draw_list.draw_calls_len = parent_draw_list.draw_calls_len + 1;
            
            // see if we need to add a new one
            if parent_draw_list.draw_calls_len > parent_draw_list.draw_calls.len(){
                parent_draw_list.draw_calls.push({
                    DrawCall{
                        draw_call_id:parent_draw_list.draw_calls.len(),
                        sub_list_id:self.draw_list_id,
                        ..Default::default()
                    }
                })
            }
            else{// or reuse a sub list node
                let draw = &mut parent_draw_list.draw_calls[id];
                draw.sub_list_id = self.draw_list_id;
            }
        }

        cx.drawing.current_draw_list_id = self.draw_list_id;
        cx.drawing.view_stack.push(self.clone());
        
        cx.turtle.begin(layout);

        false
        //cx.turtle.x = 0.0;
        //cx.turtle.y = 0.0;
    }

    pub fn end(&mut self, cx:&mut Cx){
        // we should mark the align_list to track
        // total dx/dy crossing its boundary
        // then we can incrementally redraw with that dx/dy
        // as long as we are a clipped area
        cx.drawing.view_stack.pop();
        let rect = cx.turtle.end(&mut cx.drawing);
        let draw_list = &mut cx.drawing.draw_lists[self.draw_list_id];
        draw_list.rect = rect
    }
}