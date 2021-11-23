use makepad_render::*;
use std::collections::HashMap;

pub struct Frame { // draw info per UI element
    pub view: Option<View>,
    pub components: HashMap<Id, Box<dyn LiveComponent>>
}

impl LiveNew for Frame {
    fn new(cx: &mut Cx)->Self{
        Self {
            view: None,
            components: HashMap::new()
        }
    }
}

impl LiveComponent for Frame {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(apply_from, start_index, nodes, id!(Frame));
            return nodes.skip_node(start_index);
        }
        
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            if nodes[index].id == id!(children){
                // has to be an array
                if !nodes[index].value.is_array(){
                    cx.apply_error_wrong_type_for_value(apply_from, index, nodes);
                    index = nodes.skip_node(index);
                    continue
                }
                
            }
            index = nodes.skip_node(index);
        }
        return index;
    }
}

impl Frame{
    pub fn draw_frame(&mut self, _cx:&mut Cx){
        
    }
}


