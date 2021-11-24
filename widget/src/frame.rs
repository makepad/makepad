use makepad_render::*;
use std::collections::HashMap;

live_register!{
    Frame: {{Frame}}{
    }
}

#[derive(Clone)]
pub struct FrameActionItem {
    pub id: Id,
    pub action: Box<dyn AnyAction>
}

#[derive(Clone)]
pub enum FrameActions{
    None,
    Actions(Vec<FrameActionItem>)
}

pub struct Frame { // draw info per UI element
    pub view: Option<View>,
    pub components: HashMap<Id, Box<dyn LiveComponent>>,
    pub child_list: Vec<Id>,
}
/*
#[derive(Default)]
pub struct Sink(Id);
impl LiveCast for Sink{}
impl LiveComponent for Sink{
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode])->usize{
        cx.apply_error_component_not_found(apply_from, index, nodes, self.0);
        nodes.skip_node(index)
    }
    fn type_id(&self)->std::any::TypeId{std::any::TypeId::of::<Self>()}
}*/

impl LiveCast for Frame{
    fn to_frame_component(&mut self)->Option<&mut dyn FrameComponent>{
        return Some(self);
    }
}

impl FrameComponent for Frame {
    fn handle(&mut self, cx: &mut Cx, event: &mut Event)->OptionAnyAction{
        self.handle_frame(cx, event).into()
    }
    
    fn draw(&mut self, cx: &mut Cx) {
        self.draw_frame(cx);
    }
}

impl LiveNew for Frame {
    fn new(_cx: &mut Cx)->Self{
        Self {
            //sink: Box::new(Sink::default()),
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
    fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>()}
    
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(apply_from, start_index, nodes, id!(Frame));
            return nodes.skip_node(start_index);
        }
        
        let mut index = start_index + 1;
        while index<nodes.len(){
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id{
                id!(children)=>{
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
                id!(layout)=>{
                    
                }
                _=>{
                    if !nodes[index].value.is_structy_type(){
                        cx.apply_error_no_matching_value(apply_from, index, nodes);
                    }
                }
            }
            index = nodes.skip_node(index);
        }
        return index;
    }
}


impl Frame{
    pub fn get_component(&mut self, id:Id)->Option<&mut Box<dyn LiveComponent>>{
        if let Some(comp) = self.components.get_mut(&id){
            return Some(comp)
        }
        else{
            None
        }
    }
    
    pub fn handle_frame(&mut self, cx:&mut Cx, event:&mut Event)->FrameActions{
        let mut actions = Vec::new();
        for id in &self.child_list{
            let component = self.components.get_mut(id).unwrap();
            if let Some(fc) = component.to_frame_component(){
                if let Some(action) = fc.handle(cx, event){
                    if let FrameActions::Actions(other_actions) = action.cast(){
                        actions.extend(other_actions);
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
        if actions.len()>0{
            FrameActions::Actions(actions)
        }
        else{
            FrameActions::None
        }
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

impl Default for FrameActions{
    fn default()->Self{Self::None}
}

pub struct FrameActionsIterator{
    iter: Option<std::vec::IntoIter<FrameActionItem>>
}

impl Iterator for FrameActionsIterator{
    type Item = FrameActionItem;
    fn next(&mut self)->Option<Self::Item>{
        if let Some(iter) = self.iter.as_mut(){
            return iter.next()
        }
        else{
            None
        }
    }
}

// and we'll implement IntoIterator
impl IntoIterator for FrameActions {
    type Item = FrameActionItem;
    type IntoIter = FrameActionsIterator;

    fn into_iter(self) -> Self::IntoIter {
        match self{
            Self::None=>FrameActionsIterator{iter:None},
            Self::Actions(actions)=>FrameActionsIterator{iter:Some(actions.into_iter())},
        }
    }
}

impl Into<OptionAnyAction> for FrameActions{
    fn into(self)->Option<Box<dyn AnyAction>>{
        match &self{
            Self::None=>None,
            Self::Actions(_)=>Some(Box::new(self))
        }
    }
}
