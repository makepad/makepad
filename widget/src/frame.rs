use makepad_render::*;
use std::collections::HashMap;

live_register!{
    Frame: {{Frame}}{
    }
}

pub struct Frame { // draw info per UI element
    pub view: Option<View>,
    pub components: HashMap<Id, Box<dyn LiveComponent>>,
    pub child_list: Vec<Id>,
}

impl LiveCast for Frame{
    fn to_frame_component(&mut self)->Option<&mut dyn FrameComponent>{
        return Some(self);
    }
}

impl FrameComponent for Frame {
    fn handle(&mut self, cx: &mut Cx, event: &mut Event)->Option<Box<dyn FrameAction>>{
        self.handle_frame(cx, event).into_frame_action()
    }
    
    fn draw(&mut self, cx: &mut Cx) {
        self.draw_frame(cx);
    }
}

impl LiveNew for Frame {
    fn new(_cx: &mut Cx)->Self{
        Self {
            view: None,
            components: HashMap::new(),
            child_list: Vec::new()
        }
    }
    
    fn live_register(cx: &mut Cx) {
        struct Factory();
        impl LiveFactory for Factory {
            fn new_component(&self, cx: &mut Cx) -> Box<dyn LiveComponent> where Self: Sized {
                Box::new(Frame::new(cx))
            }
        }
        cx.register_factory(Self::live_type(), Box::new(Factory()));
    }
}

impl Frame{
    fn create_component(&mut self,  cx: &mut Cx, apply_from: ApplyFrom, id:Id, live_type:LiveType, index:usize, nodes:&[LiveNode]){
        let factories = cx.live_factories.clone();
        let factories_cp = factories.borrow();
        if let Some(factory) = factories_cp.get(&live_type){
            let mut component = factory.new_component(cx);
            component.apply(cx, apply_from, index, nodes);
            self.child_list.push(id);
            self.components.insert(id, component);
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
                self.child_list.truncate(0);
                // has to be an array
                if !nodes[index].value.is_array(){
                    cx.apply_error_wrong_type_for_value(apply_from, index, nodes);
                    index = nodes.skip_node(index);
                    continue
                }

                let mut node_iter = nodes.first_child(index);
                while let Some(index) = node_iter{
                    if let LiveValue::Id(id) = nodes[index].value{
                        if let Some(index) = nodes.prev_by_name(index, id){
                            if let LiveValue::Class(live_type) = nodes[index].value{
                                self.create_component(cx, apply_from, id, live_type, index, nodes);
                            }
                            else{
                                cx.apply_error_wrong_type_for_value(apply_from, index, nodes);
                            }
                        }
                        else{
                            cx.apply_error_cant_find_target(apply_from, index, nodes, id);
                        }
                    }
                    else{
                        cx.apply_error_wrong_type_for_value(apply_from, index, nodes);
                    }
                    node_iter = nodes.next_child(index);
                } 
            }
            index = nodes.skip_node(index);
        }
        return index;
    }
}

pub struct FrameActionItem {
    pub id: Id,
    pub action: Box<dyn FrameAction>
}

pub struct FrameActions(Vec<FrameActionItem>);

impl FrameActions {
    pub fn iter(&self) -> std::slice::Iter<'_, FrameActionItem> {
        self.0.iter()
    }
    
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, FrameActionItem> {
        self.0.iter_mut()
    }
}

impl IntoFrameAction for FrameActions{
    fn into_frame_action(self)->Option<Box<dyn FrameAction>>{
        if self.0.len() == 0{
            return None
        }
        return Some(Box::new(self));
    }
}

impl Frame{
    pub fn handle_frame(&mut self, cx:&mut Cx, event:&mut Event)->FrameActions{
        let mut actions = Vec::new();
        for id in &self.child_list{
            let component = self.components.get_mut(id).unwrap();
            if let Some(fc) = component.to_frame_component(){
                if let Some(action) = fc.handle(cx, event){
                    // flatten frame events
                    if let Some(results) = action.cast::<FrameActions>(){
                        //for result in &results.0{
                        //    actions.push(result);
                        //}
                        //results.0.clone();
                        //println!("EXTENDING {}", results.0[0].id);
                        //actions.extend_from_slice(&results.0);
                    }
                    else{
                        actions.push(FrameActionItem{
                            id:*id,
                            action:action
                        });
                    }
                }
            }
        }
        FrameActions(actions)
    }    
    pub fn draw_frame(&mut self, cx:&mut Cx){
        for id in &self.child_list{
            let component = self.components.get_mut(id).unwrap();
            if let Some(fc) = component.to_frame_component(){
                fc.draw(cx)
            }
        }
    }
}


