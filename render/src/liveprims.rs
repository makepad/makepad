pub use {
    std::{
        any::TypeId,
    },
    makepad_live_compiler::*,
    crate::{
        cx::Cx,
        events::Event,
        livetraits::*,
        liveeval::*,
        animator::Animator
    }
};

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
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "LiveId");
                nodes.skip_node(index)
            }
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
            LiveValue::Expr => {
                let ret = live_eval(cx, index, apply_from, &mut (index + 1), nodes);
                match ret{
                    LiveEval::Bool(v)=>{
                        *self = v;
                    }
                    _=>{
                        cx.apply_error_wrong_expression_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "bool", ret);
                    }
                }
                nodes.skip_node(index)
            },
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "bool");
                nodes.skip_node(index)
            }
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
            LiveValue::Expr => {
                let ret = live_eval(cx, index, apply_from, &mut (index + 1), nodes);
                match ret{
                    LiveEval::Float(v)=>{
                        *self = v as f32;
                    }
                    LiveEval::Int(v)=>{
                        *self = v as f32;
                    }
                    _=>{
                        cx.apply_error_wrong_expression_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "f32", ret);
                    }
                }
                nodes.skip_node(index)
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply_from, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "f32");
                nodes.skip_node(index)
            }
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
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "f64");
                nodes.skip_node(index)
            }
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
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "i64");
                nodes.skip_node(index)
            }
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
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "Vec2");
                nodes.skip_node(index)
            }
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
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "Vec3");
                nodes.skip_node(index)
            }
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
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "Vec4");
                nodes.skip_node(index)
            }
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
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), apply_from, index, nodes, "String");
                nodes.skip_node(index)
            }
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
    LivePtr {file_id: LiveFileId(0), index: 0},
    fn apply(&mut self, _cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let Some(file_id) = apply_from.file_id() {
            self.file_id = file_id;
            self.index = index as u32;
        }
        nodes.skip_node(index)
    },
    fn to_live_value(&self) -> LiveValue {
        panic!()
    }
);

