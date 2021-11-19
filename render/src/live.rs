#![allow(unused_variables)]
use crate::cx::*;
use makepad_live_compiler::LiveValue;

#[derive(Clone, Debug)]
pub struct LiveBody {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub live_types: Vec<LiveType>
}

pub trait LiveFactory {
    fn new_component(&self, cx: &mut Cx) -> Box<dyn LiveComponent>;
    fn component_fields(&self, fields: &mut Vec<LiveField>);
}

pub trait LiveNew: LiveComponent {
    fn new(cx: &mut Cx) -> Self;
    fn new_apply(cx: &mut Cx, apply_from: ApplyFrom, index:usize, nodes:&[LiveNode])->Self;
    fn new_from_doc(cx: &mut Cx, live_doc_nodes:LiveDocNodes)->Self;
    fn live_type() -> LiveType;
    fn live_register(cx: &mut Cx);
}

pub trait ToLiveValue {
    fn to_live_value(&self) -> LiveValue;
}

pub trait LiveComponentValue {
    fn apply_value(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
}

pub trait LiveComponent {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
    fn apply_live(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, ApplyFrom::ApplyLive, 0, nodes);
    }
}

pub trait LiveAnimate {
    fn animate_to(&mut self, cx: &mut Cx, state_id: Id);
    fn handle_animation(&mut self, cx: &mut Cx, event:&mut Event);
}

#[derive(Debug, Clone, Copy)]
pub enum ApplyFrom {
    NewFromDoc {file_id: FileId}, // newed from DSL
    UpdateFromDoc {file_id: FileId}, // live DSL updated
    New, // Bare new without file info
    Animate, // from animate
    ApplyLive // called from bare apply_live() call
}

impl ApplyFrom {
    pub fn is_from_doc(&self) -> bool {
        match self {
            Self::NewFromDoc {..} => true,
            Self::UpdateFromDoc {..} => true,
            _ => false
        }
    }
    
    pub fn file_id(&self) -> Option<FileId> {
        match self {
            Self::NewFromDoc {file_id} => Some(*file_id),
            Self::UpdateFromDoc {file_id} => Some(*file_id),
            _ => None
        }
    }
}

pub trait CanvasComponent: LiveComponent {
    fn handle(&mut self, cx: &mut Cx, event: &mut Event);
    fn draw(&mut self, cx: &mut Cx);
    fn apply_draw(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply_live(cx, nodes);
        self.draw(cx);
    }
}

pub trait LiveApply {
    fn apply_value_unknown(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        nodes.skip_node(index)
    }
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_new(&mut self, _cx: &mut Cx) {}
}

pub struct LiveField {
    pub id: Id,
    pub live_type: Option<LiveType>,
    pub live_or_calc: LiveOrCalc
}

#[derive(Default)]
pub struct LiveBinding {
    pub live_ptr: Option<LivePtr>
}


impl Cx {
    pub fn live_register(&mut self) {
        crate::drawquad::live_register(self);
        crate::drawcolor::live_register(self);
        crate::drawtext::live_register(self);
        crate::geometrygen::live_register(self);
        crate::shader_std::live_register(self);
        crate::font::live_register(self);
    }
    /*
    pub fn clone_from_module_path(&self, module_path: &str) -> Option<(FileId, Vec<LiveNode>)> {
        self.shader_registry.live_registry.clone_from_module_path(module_path)
    }
    
    pub fn clone_from_ptr_name(&self, live_ptr: LivePtr, name: Id, out:&mut Vec<LiveNode >) -> bool {
        self.shader_registry.live_registry.clone_from_ptr_name(live_ptr, name, out)
    }
    
    pub fn ptr_to_nodes_index(&self, live_ptr: LivePtr) -> (&[LiveNode], usize) {
        return self.shader_registry.live_registry.ptr_to_nodes_index(live_ptr)
    }
    
    pub fn ptr_name_to_nodes_index(&self, live_ptr: LivePtr, id: Id) -> Option<(&[LiveNode], usize)> {
        let (nodes, index) = self.shader_registry.live_registry.ptr_to_nodes_index(live_ptr);
        if let Ok(index) = nodes.child_by_name(index, id) {
            return Some((nodes, index))
        }
        else {
            None
        }
    }
    */
    // ok so now what. now we should run the expansion
    pub fn live_expand(&mut self) {
        // lets expand the f'er
        let mut errs = Vec::new();
        let mut live_registry = self.live_registry.borrow_mut();
        live_registry.expand_all_documents(&mut errs);
        for err in errs {
            println!("Error expanding live file {}", live_registry.live_error_to_live_file_error(err));
        }
    }
    
    pub fn verify_type_signature(&self, live_ptr: LivePtr, live_type: LiveType) -> bool {
        let live_registry = self.live_registry.borrow();
        let node = live_registry.ptr_to_node(live_ptr);
        if let LiveValue::LiveType(ty) = node.value {
            if ty == live_type {
                return true
            }
        }
        println!("TYPE SIGNATURE VERIFY FAILED");
        false
    }
    
    pub fn register_live_body(&mut self, live_body: LiveBody) {
        let result = self.live_registry.borrow_mut().parse_live_file(
            &live_body.file,
            ModulePath::from_str(&live_body.module_path).unwrap(),
            live_body.code,
            live_body.live_types,
            live_body.line
        );
        if let Err(err) = result {
            println!("Error parsing live file {}", err);
        }
    }
    
    pub fn register_factory(&mut self, live_type: LiveType, factory: Box<dyn LiveFactory>) {
        self.live_factories.insert(live_type, factory);
    }
    
    pub fn get_factory(&mut self, live_type: LiveType) -> &Box<dyn LiveFactory> {
        self.live_factories.get(&live_type).unwrap()
    }
}


#[macro_export]
macro_rules!live_primitive {
    ( $ ty: ident, $ default: expr, $ apply: item, $ to_live_value: item) => {
        impl ToLiveValue for $ ty {
            $ to_live_value
        }
        impl LiveComponent for $ ty {
            $ apply
        }
        impl LiveNew for $ ty {
            fn new(_cx: &mut Cx) -> Self {
                $ default
            }
            fn new_apply(cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> Self {
                let mut ret = $ default;
                ret.apply(cx, apply_from, index, nodes);
                ret
            }
            fn new_from_doc(cx: &mut Cx, live_doc_nodes:LiveDocNodes) -> Self {
                let mut ret = $ default;
                ret.apply(cx, ApplyFrom::NewFromDoc{file_id:live_doc_nodes.file_id}, live_doc_nodes.index, live_doc_nodes.nodes);
                ret
            }
            fn live_type() -> LiveType {
                LiveType(std::any::TypeId::of::< $ ty>())
            }
            fn live_register(cx: &mut Cx) {
            }
        }
    }
}

live_primitive!(
    LiveValue,
    LiveValue::None,
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if nodes[index].value.is_array(){
            if let Some(value) = Animator::last_keyframe_value_from_array(index, nodes) {
                self.apply(cx, apply_from, index, nodes);
            }
            nodes.skip_node(index)
        }
        else if nodes[index].value.is_open(){ // cant use this
            nodes.skip_node(index)
        }
        else{
            *self = nodes[index].value.clone();
            index + 1
        }
    },
    fn to_live_value(&self) -> LiveValue {
        self.clone()
    }
);

live_primitive!(
    Id,
    Id::empty(),
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Id(id) => {
                *self = *id;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply_from, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Id(*self)
    }
);

live_primitive!(
    f32,
    0.0f32,
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float(val) => {
                *self = *val as f32;
                index + 1
            }
            LiveValue::Int(val) => {
                *self = *val as f32;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply_from, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Float(*self as f64)
    }
);

live_primitive!(
    f64,
    0.0f64,
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float(val) => {
                *self = *val as f64;
                index + 1
            }
            LiveValue::Int(val) => {
                *self = *val as f64;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply_from, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Float(*self as f64)
    }
);

live_primitive!(
    Vec2,
    Vec2::default(),
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Vec2(val) => {
                *self = *val;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply_from, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Vec2(*self)
    }
);

live_primitive!(
    Vec3,
    Vec3::default(),
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Vec3(val) => {
                *self = *val;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply_from, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Vec3(*self)
    }
);

live_primitive!(
    Vec4,
    Vec4::default(),
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Color(v) => {
                *self = Vec4::from_u32(*v);
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply_from, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Color(self.to_u32())
    }
);

live_primitive!(
    String,
    String::default(),
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Str(v) => {
                self.truncate(0);
                self.push_str(v);
                index + 1
            }
            LiveValue::String(v) => {
                self.truncate(0);
                self.push_str(v);
                index + 1
            }
            LiveValue::StringRef {string_start, string_count} => {
                let live_registry = cx.live_registry.borrow();
                let origin_doc = live_registry.token_id_to_origin_doc(nodes[index].token_id.unwrap());
                origin_doc.get_string(*string_start, *string_count, self);
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply_from, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::String(self.clone())
    }
);

