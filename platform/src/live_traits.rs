pub use {
    std::{
        any::TypeId,
    },
    crate::{
        makepad_live_compiler::*,
        event::Event,
        cx::Cx,
        animator::{Animator,AnimatorAction, Animate}
    }
};

pub trait LiveNewHelper {
    
}

pub fn from_ptr_impl<CB>(cx: &mut Cx, live_ptr: LivePtr, cb: CB)
where CB: FnOnce(&mut Cx, LiveFileId, usize, &[LiveNode]) -> usize {
    let live_registry_rc = cx.live_registry.clone();
    let live_registry = live_registry_rc.borrow();
    if !live_registry.generation_valid(live_ptr){
        println!("Generation invalid in new_from_ptr");
        return
    }
    let doc = live_registry.ptr_to_doc(live_ptr);

    let next_index = cb(cx, live_ptr.file_id, live_ptr.index as usize, &doc.nodes);
    if next_index <= live_ptr.index as usize + 2 {
        cx.apply_error_empty_object(live_error_origin!(), live_ptr.index as usize, &doc.nodes);
    }
}

pub trait LiveNew: LiveApply {
    fn new(cx: &mut Cx) -> Self;
    
    fn live_register(_cx: &mut Cx) {}
    
    fn live_type_info(cx: &mut Cx) -> LiveTypeInfo;
    
    fn new_apply(cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        ret.apply(cx, apply_from, index, nodes);
        ret
    }
    
    fn new_apply_mut(cx: &mut Cx, apply_from: ApplyFrom, index: &mut usize, nodes: &[LiveNode]) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        *index = ret.apply(cx, apply_from, *index, nodes);
        ret
    }
    
    fn new_from_ptr(cx: &mut Cx, live_ptr: LivePtr) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        from_ptr_impl(cx, live_ptr, |cx, file_id, index, nodes|{
            ret.apply(cx, ApplyFrom::NewFromDoc {file_id}, index, nodes)
        });
        return ret
    }
    
    fn new_from_option_ptr(cx: &mut Cx, live_ptr: Option<LivePtr>) -> Self where Self: Sized {
        let mut ret = Self::new(cx);
        if let Some(live_ptr) = live_ptr{
            from_ptr_impl(cx, live_ptr, |cx, file_id, index, nodes|{
                ret.apply(cx, ApplyFrom::NewFromDoc {file_id}, index, nodes)
            });
        }
        return ret
    }
    
    fn new_from_ptr_debug(cx: &mut Cx, live_ptr: LivePtr) -> Self where Self: Sized {
        cx.live_registry.borrow().ptr_to_doc(live_ptr).nodes.debug_print(live_ptr.index as usize, 100);
        let ret = Self::new_from_ptr(cx, live_ptr);
        return ret
    }
    
    fn new_as_main_module(cx: &mut Cx, module_path: &str, id: LiveId) -> Option<Self> where Self: Sized {
        let module_id = LiveModuleId::from_str(module_path).unwrap();
        {
            let live_registry_rc = cx.live_registry.clone();
            let mut live_registry = live_registry_rc.borrow_mut();
            if let Some(file_id) = live_registry.module_id_to_file_id.get(&module_id) {
                live_registry.main_module = Some(*file_id);
            }
        }
        Self::new_from_module(cx, module_id, id)
    }
    
    fn new_from_module(cx: &mut Cx, module_id: LiveModuleId, id: LiveId) -> Option<Self> where Self: Sized {
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        if let Some(file_id) = live_registry.module_id_to_file_id.get(&module_id) {
            let file = live_registry.file_id_to_file(*file_id);
            if let Some(index) = file.expanded.nodes.child_by_name(0, id) {
                let mut ret = Self::new(cx);
                ret.apply(cx, ApplyFrom::NewFromDoc {file_id: *file_id}, index, &file.expanded.nodes);
                return Some(ret)
            }
        }
        None
    }
}

pub trait ToLiveValue {
    fn to_live_value(&self) -> LiveValue;
}

pub trait LiveApplyValue {
    fn apply_value(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
}

pub trait LiveApply: LiveHook {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
    
    fn apply_over(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, ApplyFrom::ApplyOver, 0, nodes);
    }
    
    fn apply_clear(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, ApplyFrom::ApplyClear, 0, nodes);
    }
    
    fn handle_live_edit_event(&mut self, cx: &mut Cx, event: &Event, id: LiveId) {
        match event {
            Event::LiveEdit(live_edit_event) => {
                match live_edit_event {
                    LiveEditEvent::ReparseDocument => {
                        cx.flush_draw_shaders();
                        // ok so main_module needs a reload.
                        let live_registry_rc = cx.live_registry.clone();
                        let live_registry = live_registry_rc.borrow();
                        if let Some(file_id) = live_registry.main_module {
                            let file = live_registry.file_id_to_file(file_id);
                            if let Some(index) = file.expanded.nodes.child_by_name(0, id) {
                                self.apply(cx, ApplyFrom::UpdateFromDoc {file_id}, index, &file.expanded.nodes);
                            }
                        }
                        cx.redraw_all();
                    }
                    LiveEditEvent::Mutation {tokens, apply, live_ptrs} => {
                        cx.update_shader_tables_with_live_edit(&tokens, &live_ptrs);
                        if let Some(index) = apply.child_by_name(0, id) {
                            self.apply(cx, ApplyFrom::LiveEdit, index, &apply);
                        }
                    }
                }
            }
            _ => ()
        }
        
    }
    //fn type_id(&self) -> TypeId;
}

pub struct LiveBody {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub live_type_infos: Vec<LiveTypeInfo>
}


pub trait LiveAnimate {
    fn init_animator(&mut self, cx: &mut Cx);
    fn apply_animator(&mut self, cx: &mut Cx);
    fn toggle_animator(&mut self, cx: &mut Cx, is_state_1: bool, animate: Animate, state1: Option<LivePtr>, state2: Option<LivePtr>,) {
        if is_state_1 {
            if let Animate::Yes = animate {
                self.animate_to(cx, state1)
            }
            else {
                self.animate_cut(cx, state1)
            }
        }
        else {
            if let Animate::Yes = animate {
                self.animate_to(cx, state2)
            }
            else {
                self.animate_cut(cx, state2)
            }
        }
    }
    fn animator_is_in_state(&mut self, cx: &mut Cx, state: Option<LivePtr>) -> bool;
    fn animate_cut(&mut self, cx: &mut Cx, state: Option<LivePtr>);
    fn animate_to(&mut self, cx: &mut Cx, state: Option<LivePtr>);
    fn animator_handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> AnimatorAction;
}

#[derive(Debug, Clone, Copy)]
pub enum ApplyFrom {
    NewFromDoc {file_id: LiveFileId}, // newed from DSL
    UpdateFromDoc {file_id: LiveFileId}, // live DSL substantially updated
    
    LiveEdit, // applying a live edit mutation
    
    New, // Bare new without file info
    Animate, // from animate
    ApplyOver, // called from bare apply_live() call
    ApplyClear // called from bare apply_live() call
}

impl ApplyFrom {
    pub fn is_from_doc(&self) -> bool {
        match self {
            Self::NewFromDoc {..} => true,
            Self::UpdateFromDoc {..} => true,
            _ => false
        }
    }
    
    pub fn file_id(&self) -> Option<LiveFileId> {
        match self {
            Self::NewFromDoc {file_id} => Some(*file_id),
            Self::UpdateFromDoc {file_id} => Some(*file_id),
            _ => None
        }
    }
}


pub trait LiveHook {
    fn apply_value_unknown(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if !nodes[index].id.is_capitalised() {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_new(&mut self, _cx: &mut Cx) {}
}

impl<T> LiveHook for Option<T> where T: LiveApply + LiveNew + 'static {}
impl<T> LiveApply for Option<T> where T: LiveApply + LiveNew + 'static {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let Some(v) = self {
            v.apply(cx, apply_from, index, nodes)
        }
        else {
            let mut inner = T::new(cx);
            let index = inner.apply(cx, apply_from, index, nodes);
            *self = Some(inner);
            index
        }
    }
}

impl<T> LiveNew for Option<T> where T: LiveApply + LiveNew + 'static{
    fn new(_cx: &mut Cx) -> Self {
        Self::None
    }
    fn new_apply(cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Self {
        let mut ret = Self::None;
        ret.apply(cx, apply_from, index, nodes);
        ret
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        T::live_type_info(_cx)
    }
}
