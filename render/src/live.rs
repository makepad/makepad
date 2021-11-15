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
    //    fn live_type(&self) -> LiveType;
}

pub trait LiveNew {
    fn new(cx: &mut Cx) -> Self;
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
    fn apply_index(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
    fn apply(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply_index(cx, ApplyFrom::Apply, 0, nodes);
    }
}

#[derive(Debug,Clone,Copy)]
pub enum ApplyFrom{
    LiveNew{file_id:FileId}, // newed from DSL
    LiveUpdate{file_id:FileId}, // live DSL updated
    DataNew, // newed from bare data
    Animate,// from animate
    Apply // called from bare apply() call
}

impl ApplyFrom{
    pub fn file_id(&self)->Option<FileId>{
        match self{
            Self::LiveNew{file_id}=>Some(*file_id),
            Self::LiveUpdate{file_id}=>Some(*file_id),
            _=>None
        }
    }
}

pub trait CanvasComponent: LiveComponent {
    fn handle(&mut self, cx: &mut Cx, event: &mut Event);
    fn draw(&mut self, cx: &mut Cx);
    fn apply_draw(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, nodes);
        self.draw(cx);
    }
}

pub trait LiveComponentHooks {
    fn apply_value_unknown(&mut self, _cx: &mut Cx, _apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize {
        nodes.skip_node(index)
    }
    fn before_apply_index(&mut self, _cx: &mut Cx, _apply_from:ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_apply_index(&mut self, _cx: &mut Cx, _apply_from:ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_new(&mut self, _cx: &mut Cx) {}
}

pub enum LiveFieldKind {
    Local,
    Live,
}

pub struct LiveField {
    pub id: Id,
    pub live_type: Option<LiveType>,
    pub kind: LiveFieldKind
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
    
    pub fn clone_from_module_path(&self, module_path: &str) -> Option<(FileId,Vec<LiveNode>)> {
        self.shader_registry.live_registry.clone_from_module_path(module_path)
    }
    
    // forwards to the live registry
    /*
    pub fn live_ptr_from_module_path(&self, module_path: &str, id: Id) -> Option<LivePtr> {
        self.shader_registry.live_registry.live_ptr_from_module_path(module_path, id)
    }
    
    pub fn resolve_ptr(&self, live_ptr: LivePtr) -> &LiveNode {
        self.shader_registry.live_registry.resolve_ptr(live_ptr)
    }
    
    pub fn resolve_doc_ptr(&self, live_ptr: LivePtr) -> (&LiveDocument, &LiveNode) {
        self.shader_registry.live_registry.resolve_doc_ptr(live_ptr)
    }

    pub fn doc_from_token_id(&self, token_id: TokenId) -> &LiveDocument {
        self.shader_registry.live_registry.doc_from_token_id(token_id)
    }*/
    
    /*
    pub fn find_class_prop_ptr(&self, class_ptr: LivePtr, seek_id: Id) -> Option<LivePtr> {
        if let Some(mut iter) = self.shader_registry.live_registry.live_class_iterator(class_ptr) {
            while let Some((id, live_ptr)) = iter.next_id(&self.shader_registry.live_registry) {
                if id == seek_id {
                    return Some(live_ptr)
                }
            }
        }
        None
    }*/
    
    pub fn ptr_to_nodes_index(&self, live_ptr: LivePtr) -> (&[LiveNode], usize) {
        return self.shader_registry.live_registry.ptr_to_nodes_index(live_ptr)
    }
    
    // ok so now what. now we should run the expansion
    pub fn live_expand(&mut self) {
        // lets expand the f'er
        let mut errs = Vec::new();
        self.shader_registry.live_registry.expand_all_documents(&mut errs);
        for err in errs {
            println!("Error expanding live file {}", self.shader_registry.live_registry.live_error_to_live_file_error(err));
        }
    }
    
    pub fn verify_type_signature(&self, live_ptr: LivePtr, live_type: LiveType) -> bool {
        let node = self.shader_registry.live_registry.ptr_to_node(live_ptr);
        if let LiveValue::LiveType(ty) = node.value {
            if ty == live_type {
                return true
            }
        }
        println!("TYPE SIGNATURE VERIFY FAILED");
        false
    }
    
    pub fn register_live_body(&mut self, live_body: LiveBody) {
        let result = self.shader_registry.live_registry.parse_live_file(
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
            fn live_type() -> LiveType {
                LiveType(std::any::TypeId::of::< $ ty>())
            }
            fn live_register(cx: &mut Cx) {
            }
        }
    }
}

live_primitive!(
    KeyFrameValue,
    KeyFrameValue::None,
    fn apply_index(&mut self, _cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize {
        match &nodes[index].value {
            LiveValue::Id(id) => {
                *self = KeyFrameValue::Id(*id);
                index + 1
            }
            LiveValue::Float(f)=>{
                *self = KeyFrameValue::Float(*f);
                index + 1
            }
            LiveValue::Vec2(f)=>{
                *self = KeyFrameValue::Vec2(*f);
                index + 1
            }
            LiveValue::Vec3(f)=>{
                *self = KeyFrameValue::Vec3(*f);
                index + 1
            }
            LiveValue::Color(f)=>{
                *self = KeyFrameValue::Vec4(Vec4::from_u32(*f));
                index + 1
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        match self{
            Self::None => LiveValue::None,
            Self::Float(v)=> LiveValue::Float(*v),
            Self::Vec2(v)=> LiveValue::Vec2(*v),
            Self::Vec3(v)=> LiveValue::Vec3(*v),
            Self::Vec4(v)=> LiveValue::Color(v.to_u32()),
            Self::Id(v)=> LiveValue::Id(*v),
        }
    }
);

live_primitive!(
    Id,
    Id::empty(),
    fn apply_index(&mut self, _cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize {
        match &nodes[index].value {
            LiveValue::Id(id) => {
                *self = *id;
                index + 1
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
    fn apply_index(&mut self, _cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize{
        match &nodes[index].value {
            LiveValue::Float(val) => {
                *self = *val as f32;
                index + 1
            }
            LiveValue::Int(val) => {
                *self = *val as f32;
                index + 1
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
    fn apply_index(&mut self, _cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize{
        match &nodes[index].value {
            LiveValue::Float(val) => {
                *self = *val as f64;
                index + 1
            }
            LiveValue::Int(val) => {
                *self = *val as f64;
                index + 1
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
    fn apply_index(&mut self, _cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize{
        match &nodes[index].value {
            LiveValue::Vec2(val) => {
                *self = *val;
                index + 1
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
    fn apply_index(&mut self, _cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize{
        match &nodes[index].value {
            LiveValue::Vec3(val) => {
                *self = *val;
                index + 1
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
    fn apply_index(&mut self, _cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize{
        match &nodes[index].value {
            LiveValue::Color(v) => {
                *self = Vec4::from_u32(*v);
                index + 1
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
    fn apply_index(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode])->usize{
        match &nodes[index].value {
            LiveValue::Str(v) => {
                *self = v.to_string();
                index + 1
            }
            LiveValue::String(v) => {
                *self = v.clone();
                index + 1
            }
            LiveValue::StringRef{string_start, string_count} => {
                let origin_doc = cx.shader_registry.live_registry.token_id_to_origin_doc(nodes[index].token_id.unwrap());
                origin_doc.get_string(*string_start, *string_count, self);
                index + 1
            }
            _ => nodes.skip_node(index)
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::String(self.clone())
    }
);

