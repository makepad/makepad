#![allow(unused_variables)]
use crate::cx::*;
use makepad_live_compiler::LiveValue;
use makepad_live_compiler::LiveError;
use makepad_live_compiler::LiveErrorOrigin;
use makepad_live_compiler::LiveTypeInfo;
use makepad_live_compiler::ModulePath;
use makepad_live_compiler::live_error_origin;

pub struct LiveBody {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub live_type_infos: Vec<LiveTypeInfo>
}

pub trait LiveFactory {
    fn new_component(&self, cx: &mut Cx) -> Box<dyn LiveComponent>;
}

pub trait LiveNew: LiveComponent {
    fn new(cx: &mut Cx) -> Self;
    
    fn live_register(cx: &mut Cx){}
    
    fn live_type_info() -> LiveTypeInfo where Self:Sized + 'static{
        LiveTypeInfo{module_path:ModulePath::from_str(&module_path!()).unwrap(), live_type:Self::live_type(), fields:Vec::new(),
               type_name:Id::from_str("LiveNew").unwrap()}
    }

    fn new_apply(cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> Self where Self:Sized{
      let mut ret = Self::new(cx);
      ret.apply(cx, apply_from, index, nodes);
      ret
    }
    
    fn new_apply_mut(cx: &mut Cx, apply_from:ApplyFrom, index:&mut usize, nodes:&[LiveNode]) -> Self where Self:Sized{
      let mut ret = Self::new(cx);
      *index = ret.apply(cx, apply_from, *index, nodes);
      ret
    }
    
    fn new_from_doc(cx: &mut Cx, live_doc_nodes: LiveDocNodes)->Self where Self:Sized{
        let mut ret = Self::new(cx);
        ret.apply(cx, ApplyFrom::NewFromDoc {file_id: live_doc_nodes.file_id}, live_doc_nodes.index, live_doc_nodes.nodes);
        ret
    }

    fn live_type() -> LiveType where Self:'static{
         LiveType(std::any::TypeId::of::<Self>())
    }
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
    fn handle_animation(&mut self, cx: &mut Cx, event: &mut Event);
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
    fn apply_value_unknown(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if nodes[index].value.is_value_type() {
            cx.apply_error_no_matching_value(apply_from, index, nodes);
        }
        nodes.skip_node(index)
    }
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_new(&mut self, _cx: &mut Cx) {}
}

/*
#[derive(Default)]
pub struct LiveBinding {
    pub live_ptr: Option<LivePtr>
}

*/
impl Cx {
    pub fn live_register(&mut self) {
        crate::drawquad::live_register(self);
        crate::drawcolor::live_register(self);
        crate::drawtext::live_register(self);
        crate::geometrygen::live_register(self);
        crate::shader_std::live_register(self);
        crate::font::live_register(self);
    }
    
    pub fn apply_error_tuple_enum_arg_not_found(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: Id, base: Id, arg: usize) {
        self.apply_error(apply_from, index, nodes, format!("tuple enum too many args for {}::{} arg no {}", enum_id, base, arg))
    }
    
    pub fn apply_error_named_enum_invalid_prop(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: Id, base: Id, prop: Id) {
        self.apply_error(apply_from, index, nodes, format!("named enum invalid property for {}::{} prop: {}", enum_id, base, prop))
    }
    
    pub fn apply_error_wrong_enum_base(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: Id, base: Id) {
        self.apply_error(apply_from, index, nodes, format!("wrong enum base expected: {} got: {}", enum_id, base))
    }
    
    pub fn apply_error_wrong_struct_name(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], struct_id: Id, got_id: Id) {
        self.apply_error(apply_from, index, nodes, format!("wrong struct name expected: {} got: {}", struct_id, got_id))
    }
    
    pub fn apply_error_wrong_type_for_struct(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], struct_id: Id) {
        self.apply_error(apply_from, index, nodes, format!("wrong type for struct: {}", struct_id))
    }
    
    pub fn apply_error_wrong_enum_variant(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: Id, variant: Id) {
        self.apply_error(apply_from, index, nodes, format!("wrong enum variant for enum: {} got variant: {}", enum_id, variant))
    }
    
    pub fn apply_error_expected_enum(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(apply_from, index, nodes, format!("expected enum value type, but got {} {:?}", nodes[index].id, nodes[index].value))
    }
    
    pub fn apply_error_no_matching_value(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(apply_from, index, nodes, format!("no matching value {}", nodes[index].id))
    }

    pub fn apply_error_wrong_type_for_value(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(apply_from, index, nodes, format!("wrong type for value {}", nodes[index].id))
    }
    
    pub fn apply_error(&mut self, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], message: String) {
        let live_registry = self.live_registry.borrow();
        if let Some(token_id) = &nodes[index].token_id {
            let err = LiveError {
                origin: live_error_origin!(),
                message,
                span: live_registry.token_id_to_span(*token_id)
            };
            println!("Apply error: {} {:?}", live_registry.live_error_to_live_file_error(err), nodes[index].value);
        }
        else {
            println!("Apply without file, at index {} {}", index, message);
            
        }
    }
    
    
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
    /*
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
    }*/
    
    pub fn register_live_body(&mut self, live_body: LiveBody) {
        //println!("START");
        let result = self.live_registry.borrow_mut().parse_live_file(
            &live_body.file,
            ModulePath::from_str(&live_body.module_path).unwrap(),
            live_body.code,
            live_body.live_type_infos,
            live_body.line
        );
        //println!("END");
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

impl<T> LiveComponent for Option<T> where T: LiveComponent + LiveNew {
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

impl<T> LiveNew for Option<T> where T: LiveComponent + LiveNew + 'static {
    fn new(_cx: &mut Cx) -> Self {
        Self::None
    }
    fn new_apply(cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Self {
        let mut ret = Self::None;
        ret.apply(cx, apply_from, index, nodes);
        ret
    }
    fn new_from_doc(cx: &mut Cx, live_doc_nodes: LiveDocNodes) -> Self {
        let mut ret = Self::None;
        ret.apply(cx, ApplyFrom::NewFromDoc {file_id: live_doc_nodes.file_id}, live_doc_nodes.index, live_doc_nodes.nodes);
        ret
    }

    fn live_type_info() -> LiveTypeInfo {
        T::live_type_info()
    }
    fn live_register(cx: &mut Cx) {
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

            fn live_type_info() -> LiveTypeInfo {
                LiveTypeInfo {
                    module_path: ModulePath::from_str(&module_path!()).unwrap(),
                    live_type: Self::live_type(),
                    fields: Vec::new(),
                    type_name: Id::from_str(stringify!( $ ty)).unwrap()
                }
            }
        }
    }
}

live_primitive!(
    LiveValue,
    LiveValue::None,
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if nodes[index].value.is_array() {
            if let Some(value) = Animator::last_keyframe_value_from_array(index, nodes) {
                self.apply(cx, apply_from, index, nodes);
            }
            nodes.skip_node(index)
        }
        else if nodes[index].value.is_open() { // cant use this
            nodes.skip_node(index)
        }
        else {
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
    bool,
    false,
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Bool(val) => {
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
        LiveValue::Bool(*self)
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
            LiveValue::FittedString(v) => {
                self.truncate(0);
                self.push_str(v.as_str());
                index + 1
            }
            LiveValue::InlineString(v) => {
                self.truncate(0);
                self.push_str(v.as_str());
                index + 1
            }
            LiveValue::DocumentString {string_start, string_count} => {
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
        // lets check our byte size and choose a storage mode appropriately.
        let bytes = self.as_bytes();
        if let Some(inline_str) = InlineString::from_str(&self){
            LiveValue::InlineString(inline_str)
        }
        else{
            LiveValue::FittedString(FittedString::from_string(self.clone()))
        }
    }
);

