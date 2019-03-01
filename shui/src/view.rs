use crate::cx::*;

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
            if cx.draw_lists_free.len() != 0{
                self.draw_list_id = cx.draw_lists_free.pop().unwrap();
            }
            else{
                self.draw_list_id = cx.draw_lists.len();
                cx.draw_lists.push(DrawList{..Default::default()});
            }
            self.initialized = true;
            let draw_list = &mut cx.draw_lists[self.draw_list_id];
            draw_list.initialize();
        }
        else{
            // set len to 0
            let draw_list = &mut cx.draw_lists[self.draw_list_id];
            draw_list.draw_calls_len = 0;
        }
        // push ourselves up the parent draw_stack
        if let Some(parent_view) = cx.view_stack.last(){

            // we need a new draw
            let parent_draw_list = &mut cx.draw_lists[parent_view.draw_list_id];

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

        cx.current_draw_list_id = self.draw_list_id;
        cx.view_stack.push(self.clone());
        
        cx.begin_turtle(layout);

        false
        //cx.turtle.x = 0.0;
        //cx.turtle.y = 0.0;
    }

    pub fn end(&mut self, cx:&mut Cx){
        cx.view_stack.pop();
        let rect = cx.end_turtle();
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        draw_list.rect = rect
    }
}