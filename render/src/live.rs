pub use {
    std::{
        any::TypeId,
    },
    makepad_live_compiler::*,
    crate::{
        cx::Cx,
        events::Event,
        animation::Animator
    }
};

pub trait LiveFactory {
    fn new_component(&self, cx: &mut Cx) -> Box<dyn LiveApply>;
}

pub trait LiveNew: LiveApply {
    fn new(cx: &mut Cx) -> Self;
    
    fn live_register(_cx: &mut Cx) {}
    
    fn live_type_info() -> LiveTypeInfo;
    
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
    
    fn new_from_ptr(cx: &mut Cx, live_ptr:LivePtr) -> Self where Self: Sized {
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        let doc = live_registry.ptr_to_doc(live_ptr);
        let mut ret = Self::new(cx);
        ret.apply(cx, ApplyFrom::NewFromDoc{file_id:live_ptr.file_id}, live_ptr.index as usize, &doc.nodes);
        return ret
    }
    
    fn new_from_module_path_id(cx: &mut Cx, module_path: &str, id:LiveId) -> Option<Self> where Self: Sized {
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        if let Some(file_id) = live_registry.module_id_to_file_id.get(&LiveModuleId::from_str(module_path).unwrap()) {
            let doc = live_registry.file_id_to_doc(*file_id);
            if let Some(index) = doc.nodes.child_by_name(0,id){
                let mut ret = Self::new(cx);
                ret.apply(cx, ApplyFrom::NewFromDoc {file_id:*file_id}, index, &doc.nodes);
                return Some(ret)
            }
        }
        None
    }
    
    fn live_type() -> LiveType where Self: 'static {
        LiveType(std::any::TypeId::of::<Self>())
    }
}

pub trait ToLiveValue {
    fn to_live_value(&self) -> LiveValue;
}

pub trait LiveApplyValue {
    fn apply_value(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
}

pub trait LiveApply : LiveHook {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
    fn apply_over(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, ApplyFrom::ApplyOver, 0, nodes);
    }
    fn apply_clear(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply(cx, ApplyFrom::ApplyClear, 0, nodes);
    }
    fn type_id(&self) -> TypeId;
}


pub trait LiveAnimate {
    fn animate_to(&mut self, cx: &mut Cx, track: LiveId, state:LivePtr);
    fn handle_animation(&mut self, cx: &mut Cx, event: &mut Event);
}

#[derive(Debug, Clone, Copy)]
pub enum ApplyFrom {
    NewFromDoc {file_id: LiveFileId}, // newed from DSL
    UpdateFromDoc {file_id: LiveFileId}, // live DSL updated
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
    fn apply_value_unknown(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let ApplyFrom::Animate = apply_from{
            if nodes[index].id == id!(from){
                return nodes.skip_node(index)
            }
        }
        cx.apply_error_no_matching_field(apply_from, index, nodes);
        nodes.skip_node(index)
    }
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {}
    fn after_new(&mut self, _cx: &mut Cx) {}
    fn to_frame_component(&mut self) -> Option<&mut dyn FrameComponent> {
        None
    }
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
    
    pub fn apply_error_tuple_enum_arg_not_found(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId, arg: usize) {
        self.apply_error(apply_from, index, nodes, format!("tuple enum too many args for {}::{} arg no {}", enum_id, base, arg))
    }
    
    pub fn apply_error_named_enum_invalid_prop(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId, prop: LiveId) {
        self.apply_error(apply_from, index, nodes, format!("named enum invalid property for {}::{} prop: {}", enum_id, base, prop))
    }
    
    pub fn apply_error_wrong_enum_base(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: LiveId, base: LiveId) {
        self.apply_error(apply_from, index, nodes, format!("wrong enum base expected: {} got: {}", enum_id, base))
    }
    
    pub fn apply_error_wrong_struct_name(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], struct_id: LiveId, got_id: LiveId) {
        self.apply_error(apply_from, index, nodes, format!("wrong struct name expected: {} got: {}", struct_id, got_id))
    }
    
    pub fn apply_error_wrong_type_for_struct(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], struct_id: LiveId) {
        self.apply_error(apply_from, index, nodes, format!("wrong type for struct: {}", struct_id))
    }
    
    pub fn apply_error_wrong_enum_variant(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], enum_id: LiveId, variant: LiveId) {
        self.apply_error(apply_from, index, nodes, format!("wrong enum variant for enum: {} got variant: {}", enum_id, variant))
    }
    
    pub fn apply_error_expected_enum(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(apply_from, index, nodes, format!("expected enum value type, but got {} {:?}", nodes[index].id, nodes[index].value))
    }
    
    pub fn apply_error_no_matching_field(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(apply_from, index, nodes, format!("no matching field: {}", nodes[index].id))
    }
    
    pub fn apply_error_wrong_type_for_value(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.apply_error(apply_from, index, nodes, format!("wrong type for value: {}", nodes[index].id))
    }
    
    pub fn apply_error_component_not_found(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], id: LiveId) {
        self.apply_error(apply_from, index, nodes, format!("component not found: {}", id))
    }
    
    pub fn apply_error_cant_find_target(&mut self, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], id: LiveId) {
        self.apply_error(apply_from, index, nodes, format!("cant find target: {}", id))
    }
        
    pub fn apply_error_wrong_animation_track_used(&mut self, index: usize, nodes: &[LiveNode], id:LiveId, expect:LiveId, got:LiveId) {
        self.apply_error(ApplyFrom::Animate, index, nodes, format!("encountered value [{}] with track [{}] whilst animating on track [{}]", id, expect, got))
    }
    
    pub fn apply_error_animate_to_unknown_track(&mut self, index: usize, nodes: &[LiveNode], id:LiveId, state_id:LiveId) {
        self.apply_error(ApplyFrom::Animate, index, nodes, format!("unknown track {} in animate_to state_id {}", id, state_id))
    }
    
    pub fn apply_key_frame_cannot_be_interpolated(&mut self, index: usize, nodes: &[LiveNode], a:&LiveValue, b:&LiveValue) {
        self.apply_error(ApplyFrom::Animate, index, nodes, format!("key frame values cannot be interpolated {:?} {:?}", a, b))
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
            LiveModuleId::from_str(&live_body.module_path).unwrap(),
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
        self.live_factories.borrow_mut().insert(live_type, factory);
    }
    
}

pub struct LiveBody {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub live_type_infos: Vec<LiveTypeInfo>
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
    fn type_id(&self) -> TypeId {
        std::any::TypeId::of::<T>()
    }
}

impl<T> LiveNew for Option<T> where T: LiveApply + LiveNew + 'static {
    fn new(_cx: &mut Cx) -> Self {
        Self::None
    }
    fn new_apply(cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Self {
        let mut ret = Self::None;
        ret.apply(cx, apply_from, index, nodes);
        ret
    }
    
    fn live_type_info() -> LiveTypeInfo {
        T::live_type_info()
    }
    fn live_register(_cx: &mut Cx) {
    }
}

#[macro_export]
macro_rules!get_component {
    ( $ comp_id: expr, $ ty: ty, $ frame: expr) => {
        $ frame.get_component( $ comp_id).map_or(None, | v | v.cast_mut::< $ ty>())
    }
}
/*
#[macro_export]
macro_rules!module_path_obj {
    () => {
        ModulePath::from_str(&module_path!()).unwrap()
    }
}*/

/*
#[macro_export]
macro_rules!let_action {
    ( $ item: expr, $ comp_id: expr, $ ty: expr) => {
        let ($comp_id, $ty) = $item.action.cast_id($item.id)
    }
}*/

#[macro_export]
macro_rules!live_primitive {
    ( $ ty: ident, $ default: expr, $ apply: item, $ to_live_value: item) => {
        impl LiveHook for $ ty {}
        impl ToLiveValue for $ ty {
            $ to_live_value
        }
        impl LiveApply for $ ty {
            fn type_id(&self) -> TypeId {
                TypeId::of::< $ ty>()
            }
            
            $ apply
        }
        impl LiveNew for $ ty {
            fn new(_cx: &mut Cx) -> Self {
                $ default
            }
            
            fn live_type_info() -> LiveTypeInfo {
                LiveTypeInfo {
                    module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
                    live_type: Self::live_type(),
                    fields: Vec::new(),
                    type_name: LiveId::from_str(stringify!( $ ty)).unwrap(),
                    kind: LiveTypeKind::Primitive
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
            if let Some(_) = Animator::last_keyframe_value_from_array(index, nodes) {
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
    LiveId,
    LiveId::empty(),
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
            LiveValue::Int(val) => {
                *self = *val != 0;
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
            LiveValue::Expr=>{
                println!("EXPR!");
                nodes.skip_node(index)
            },
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
    i64,
    0i64,
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float(val) => {
                *self = *val as i64;
                index + 1
            }
            LiveValue::Int(val) => {
                *self = *val as i64;
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
        LiveValue::Int(*self)
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
        //let bytes = self.as_bytes();
        if let Some(inline_str) = InlineString::from_str(&self) {
            LiveValue::InlineString(inline_str)
        }
        else {
            LiveValue::FittedString(FittedString::from_string(self.clone()))
        }
    }
);


live_primitive!(
    LivePtr,
    LivePtr{file_id:LiveFileId(0), index:0},
    fn apply(&mut self, _cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let Some(file_id) = apply_from.file_id(){
            self.file_id = file_id;
            self.index = index as u32;
        }
        nodes.skip_node(index)
    },
    fn to_live_value(&self) -> LiveValue {
        panic!()
    }
);

impl dyn LiveApply {
    pub fn is<T: LiveApply + 'static >(&self) -> bool {
        let t = TypeId::of::<T>();
        let concrete = self.type_id();
        t == concrete
    }
    pub fn cast<T: LiveApply + 'static >(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe {&*(self as *const dyn LiveApply as *const T)})
        } else {
            None
        }
    }
    pub fn cast_mut<T: LiveApply + 'static >(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe {&mut *(self as *const dyn LiveApply as *mut T)})
        } else {
            None
        }
    }
}

pub trait AnyAction: 'static {
    fn type_id(&self) -> TypeId;
    fn box_clone(&self) -> Box<dyn AnyAction>;
}

impl<T: 'static + ? Sized + Clone> AnyAction for T {
    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    
    fn box_clone(&self) -> Box<dyn AnyAction> {
        Box::new((*self).clone())
    }
}

impl dyn AnyAction {
    pub fn is<T: AnyAction >(&self) -> bool {
        let t = TypeId::of::<T>();
        let concrete = self.type_id();
        t == concrete
    }
    pub fn cast<T: AnyAction + Default + Clone>(&self) -> T {
        if self.is::<T>() {
            unsafe {&*(self as *const dyn AnyAction as *const T)}.clone()
        } else {
            T::default()
        }
    }
    
    pub fn cast_id<T: AnyAction + Default + Clone>(&self, id: LiveId) -> (LiveId, T) {
        if self.is::<T>() {
            (id, unsafe {&*(self as *const dyn AnyAction as *const T)}.clone())
        } else {
            (id, T::default())
        }
    }
    
}

pub type OptionAnyAction = Option<Box<dyn AnyAction >>;

impl Clone for Box<dyn AnyAction> {
    fn clone(&self) -> Box<dyn AnyAction> {
        self.as_ref().box_clone()
    }
}

pub trait FrameComponent: LiveApply {
    fn handle_event_dyn(&mut self, cx: &mut Cx, event: &mut Event) -> Option<Box<dyn AnyAction >>;
    fn draw_dyn(&mut self, cx: &mut Cx);
    fn apply_draw(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply_over(cx, nodes);
        self.draw_dyn(cx);
    }
}

