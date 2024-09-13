use {
    crate::{
        makepad_live_compiler::*,
        cx::Cx,
        scope::Scope,
    }
};

pub use crate::live_cx::LiveBody;

pub trait LiveRegister{
    fn live_register(_cx:&mut Cx){}
}

pub trait LiveHook {
    //fn before_live_design(_cx:&mut Cx){}
        
    fn apply_value_unknown(&mut self, cx: &mut Cx, _apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        if !nodes[index].origin.node_has_prefix() {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }
    
    fn skip_apply_animator(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode])->bool{
        false
    }

    fn apply_value_instance(&mut self, _cx: &mut Cx, _apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        nodes.skip_node(index)
    }
    
    fn skip_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode])->Option<usize>{None}
    fn before_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]){}
    fn after_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {}
    fn after_apply_from(&mut self, cx: &mut Cx, apply: &mut Apply) {
        match &apply.from{
            ApplyFrom::NewFromDoc{..}=>{self.after_new_from_doc(cx);self.after_apply_from_doc(cx);}
            ApplyFrom::UpdateFromDoc{..}=>{self.after_update_from_doc(cx);self.after_apply_from_doc(cx);}
            _=>()
        }
    }
    fn after_new_from_doc(&mut self, _cx:&mut Cx){}
    fn after_update_from_doc(&mut self, _cx:&mut Cx){}
    fn after_apply_from_doc(&mut self, _cx:&mut Cx){}
    fn after_new_before_apply(&mut self, _cx: &mut Cx) {}
}

pub trait LiveHookDeref {
    fn deref_before_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]){}
    fn deref_after_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]){}
}

pub trait LiveNew: LiveApply {
    fn new(cx: &mut Cx) -> Self;
    
    fn live_design_with(_cx: &mut Cx) {}
    
    fn live_type_info(cx: &mut Cx) -> LiveTypeInfo;
    
    fn new_apply(cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        ret.apply(cx, apply, index, nodes);
        ret
    }
    
    fn new_apply_over(cx: &mut Cx, nodes: &[LiveNode]) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        ret.apply_over(cx, nodes);
        ret
    }
    
    fn new_apply_mut_index(cx: &mut Cx, apply: &mut Apply, index: &mut usize, nodes: &[LiveNode]) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        *index = ret.apply(cx, apply, *index, nodes);
        ret
    }

    fn new_from_ptr(cx: &mut Cx, live_ptr: Option<LivePtr>) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        if let Some(live_ptr) = live_ptr{ 
            cx.get_nodes_from_live_ptr(live_ptr, |cx, file_id, index, nodes|{
                ret.apply(cx, &mut ApplyFrom::NewFromDoc {file_id}.into(), index, nodes)
            });
        }
        return ret
    }
    
    fn apply_from_ptr(&mut self, cx: &mut Cx, live_ptr: Option<LivePtr>) {
        if let Some(live_ptr) = live_ptr{
            cx.get_nodes_from_live_ptr(live_ptr, |cx, _file_id, index, nodes|{
                self.apply(cx, &mut ApplyFrom::Over.into(), index, nodes)
            });
        }
    }
    
    fn new_from_ptr_with_scope<'a> (cx: &mut Cx, scope:&'a mut Scope, live_ptr: Option<LivePtr>) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        if let Some(live_ptr) = live_ptr{
            cx.get_nodes_from_live_ptr(live_ptr, |cx, file_id, index, nodes|{
                ret.apply(cx, &mut ApplyFrom::NewFromDoc {file_id}.with_scope(scope), index, nodes)
            });
        }
        return ret
    }
    
    fn new_main(cx: &mut Cx) -> Self where Self: Sized {
        let lti = Self::live_type_info(cx);
        Self::new_from_module(cx, lti.module_id, lti.type_name).unwrap()
    }
    
    fn register_main_module(cx: &mut Cx) {
        let lti = Self::live_type_info(cx);
        {
            let live_registry_rc = cx.live_registry.clone();
            let mut live_registry = live_registry_rc.borrow_mut();
            live_registry.main_module = Some(lti.clone());
        }
    }
    
    fn update_main(&mut self, cx:&mut Cx){
        let lti = {
            let live_registry_rc = cx.live_registry.clone();
            let live_registry = live_registry_rc.borrow_mut();
            live_registry.main_module.as_ref().unwrap().clone()
        };
        self.update_from_module(cx, lti.module_id, lti.type_name);
    }
    
    fn new_local(cx: &mut Cx) -> Self where Self: Sized {
        let lti = Self::live_type_info(cx);
        Self::new_from_module(cx, lti.module_id, lti.type_name).unwrap()
    }
    
    fn new_from_module(cx: &mut Cx, module_id: LiveModuleId, id: LiveId) -> Option<Self> where Self: Sized {
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        if let Some(file_id) = live_registry.module_id_to_file_id.get(&module_id) {
            let file = live_registry.file_id_to_file(*file_id);
            if let Some(index) = file.expanded.nodes.child_by_name(0, id.as_instance()) {
                let mut ret = Self::new(cx);
                ret.apply(cx, &mut ApplyFrom::NewFromDoc {file_id: *file_id}.into(), index, &file.expanded.nodes);
                return Some(ret)
            }
        }
        None
    }
    
    fn update_from_module(&mut self, cx: &mut Cx, module_id: LiveModuleId, id: LiveId)  {
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        if let Some(file_id) = live_registry.module_id_to_file_id.get(&module_id) {
            let file = live_registry.file_id_to_file(*file_id);
            if let Some(index) = file.expanded.nodes.child_by_name(0, id.as_instance()) {
                self.apply(cx, &mut ApplyFrom::UpdateFromDoc {file_id: *file_id}.into(), index, &file.expanded.nodes);
            }
        }
    }
}

pub trait ToLiveValue {
    fn to_live_value(&self) -> LiveValue;
}

pub trait LiveApplyValue {
    fn apply_value(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize;
}

pub trait LiveApplyReset { 
    fn apply_reset(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]);
}

pub trait LiveApply {
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize;
    
    fn apply_over(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, &mut ApplyFrom::Over.into(), 0, nodes);
    }
}

pub trait LiveRead{
    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>);
    fn live_read(&self)->Vec<LiveNode>{
        let mut out = Vec::new();
        self.live_read_to(LiveId(0),&mut out);
        out
    }
    
}

impl<T, const N:usize> LiveRead for [T;N]  where T: LiveRead {
    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){
        out.open_array(id);
        for i in 0..N{
            self[i].live_read_to(LiveId(i as u64), out);
        }
        out.close();
    }
} 

pub struct Apply<'a,'b,'c> {
    pub from: ApplyFrom,
    pub scope: Option<&'c mut Scope<'a,'b>>,
}

impl <'a,'b,'c> Apply<'a,'b,'c>{
    pub fn override_from<F, R>(&mut self, from:ApplyFrom, f: F) -> R where F: FnOnce(&mut Apply) -> R{
        if let Some(scope) = &mut self.scope{
            f(&mut Apply{
                from: from,
                scope: Some(*scope)
            })
        }
        else{
            f(&mut Apply{
                from: from,
                scope: None
            })
            
        }
    }
}

impl ApplyFrom{
    pub fn with_scope<'a, 'b, 'c>(self, scope:&'c mut Scope<'a,'b>)->Apply<'a, 'b, 'c>{
        Apply{
            from: self,
            scope: Some(scope)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ApplyFrom {
    NewFromDoc {file_id: LiveFileId}, // newed from DSL
    UpdateFromDoc {file_id: LiveFileId}, // live DSL substantially updated
        
    New, // Bare new without file info
    Animate, // from animate
    AnimatorInit,
    Over, // called from bare apply_live() call
}

impl<'a,'b, 'c> From<ApplyFrom> for Apply<'a,'b,'c> {
    fn from(from: ApplyFrom) -> Self {
        Self {
            from,
            scope: None,
        }
    }
}

impl ApplyFrom {
    pub fn is_from_doc(&self) -> bool {
        match self {
            Self::NewFromDoc {..} => true,
            Self::UpdateFromDoc {..} => true,
            _ => false
        }
    }

    pub fn is_new_from_doc(&self) -> bool {
        match self {
            Self::NewFromDoc {..} => true,
            _ => false
        }
    }
    
    pub fn should_apply_reset(&self) -> bool {
        match self {
            Self::UpdateFromDoc{..}  => true,
            _ => false
        }
    }
    
    pub fn is_update_from_doc(&self) -> bool {
        match self {
            Self::UpdateFromDoc {..} => true,
            _ => false
        }
    }
        
    pub fn file_id(&self) -> Option<LiveFileId> {
        match self {
            Self::NewFromDoc {file_id} => Some(*file_id),
            Self::UpdateFromDoc {file_id,..} => Some(*file_id),
            _ => None
        }
    }
    
    pub fn to_live_ptr(&self, cx:&Cx, index:usize) -> Option<LivePtr> {
        if let Some(file_id) = self.file_id(){
            let live_ptr = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
            return Some(live_ptr)
        }
        None
    }
        
        
}


impl<T> LiveHook for Option<T> where T: LiveApply + LiveNew + 'static {}
impl<T> LiveApply for Option<T> where T: LiveApply + LiveNew + 'static {
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        if let Some(v) = self {
            v.apply(cx, apply, index, nodes)
        }
        else {
            let mut inner = T::new(cx);
            let index = inner.apply(cx, apply, index, nodes);
            *self = Some(inner);
            index
        }
    }
} 

impl<T> LiveNew for Option<T> where T: LiveApply + LiveNew + 'static{
    fn new(_cx: &mut Cx) -> Self {
        Self::None
    }
    fn new_apply(cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> Self {
        let mut ret = Self::None;
        ret.apply(cx, apply, index, nodes);
        ret
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        T::live_type_info(_cx)
    }
}


impl<T> LiveHook for Vec<T> where T: LiveApply + LiveNew + 'static {}
impl<T> LiveApply for Vec<T> where T: LiveApply + LiveNew + 'static {
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        // we can only apply from an Array
        self.clear();
        if nodes[index].is_array(){
            let mut index = index + 1;
            loop{
                if nodes[index].is_close(){
                    index += 1;
                    break;
                }
                let mut inner = T::new(cx);
                index = inner.apply(cx, apply, index, nodes);
                self.push(inner);
            }
            index
        }
        else{
            cx.apply_error_expected_array(live_error_origin!(), index, nodes);
            nodes.skip_node(index)
        }
    }
} 

impl<T> LiveNew for Vec<T> where T: LiveApply + LiveNew + 'static{
    fn new(_cx: &mut Cx) -> Self {
        Vec::new()
    }
    fn new_apply(cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> Self {
        let mut ret = Vec::new();
        ret.apply(cx, apply, index, nodes);
        ret
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        T::live_type_info(_cx)
    }
}


impl<T, const N:usize> LiveHook for [T;N] where T: LiveApply + LiveNew + 'static{}
impl<T, const N:usize> LiveApply for [T;N]  where T: LiveApply + LiveNew + 'static {
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        // we can only apply from an Array
        if nodes[index].is_array(){
            let mut index = index + 1;
            let mut count = 0;
            loop{
                if nodes[index].is_close(){
                    index += 1;
                    break;
                }
                if count < self.len(){
                    index = self[count].apply(cx, apply, index, nodes);
                    count += 1;
                }
                else{
                   index = nodes.skip_node(index)
                }
            }
            index
        }
        else{
            cx.apply_error_expected_array(live_error_origin!(), index, nodes);
            nodes.skip_node(index)
        }
    }
} 

impl<T, const N:usize> LiveNew for [T;N] where T: LiveApply + LiveNew + 'static{
    fn new(cx: &mut Cx) -> Self {
        std::array::from_fn(|_| T::new(cx))
    }
    
    fn new_apply(cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> Self {
        let mut ret = Self::new(cx);
        ret.apply(cx, apply, index, nodes);
        ret
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        T::live_type_info(_cx)
    }
}




