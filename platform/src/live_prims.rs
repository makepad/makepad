use {
    std::{str, sync::Arc},
    crate::{
        makepad_live_compiler::*,
        makepad_math::*,
        cx::Cx,
        live_traits::*,
        animator::Animator
    }
};

#[macro_export]
macro_rules!get_component {
    ( $ comp_id: expr, $ ty: ty, $ frame: expr) => {
        $ frame.get_component( $ comp_id).map_or(None, | v | v.cast_mut::< $ ty>())
    }
}
 
#[macro_export]
macro_rules!live_primitive {
    ( $ ty: ty, $ default: expr, $ apply: item, $ to_live_value: item) => {
        impl LiveHook for $ ty {}
        impl ToLiveValue for $ ty {
            $ to_live_value
        }
        impl LiveRead for $ ty {
            fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){
                out.push(LiveNode::from_id_value(id, self.to_live_value()));
            } 
        }
        impl LiveApply for $ ty {
            //fn type_id(&self) -> TypeId {
            //    TypeId::of::< $ ty>()
            // }
            $ apply
        }
        impl LiveNew for $ ty {
            fn live_design_with(_cx:&mut Cx){}
            fn new(_cx: &mut Cx) -> Self {
                $ default
            }
            
            fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
                LiveTypeInfo {
                    module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
                    live_type: LiveType::of::<Self>(),
                    fields: Vec::new(),
                    live_ignore: true,
                    type_name: LiveId::from_str_with_lut(stringify!( $ ty)).unwrap(),
                    //kind: LiveTypeKind::Primitive
                }
            }
        }
    }
}

live_primitive!(
    LiveValue,
    LiveValue::None,
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        if nodes[index].is_array() {
            if let Some(_) = Animator::last_keyframe_value_from_array(index, nodes) {
                self.apply(cx, apply, index, nodes);
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
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Id(id) => {
                *self = *id;
                index + 1
            }
            LiveValue::BareEnum(id)=>{
                *self = *id;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "LiveId");
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
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Bool(val) => {
                *self = *val;
                index + 1
            }
            LiveValue::Uint64(val) => {
                *self = *val != 0;
                index + 1
            }
            LiveValue::Int64(val) => {
                *self = *val != 0;
                index + 1
            }
            LiveValue::Float64(val) => {
                *self = val.abs()>0.00001;
                index + 1
            }
            LiveValue::Float32(val) => {
                *self = val.abs()>0.00001;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "bool");
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
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float32(val) => {
                *self = *val;
                index + 1
            }
            LiveValue::Float64(val) => {
                *self = *val as f32;
                index + 1
            }
            LiveValue::Uint64(val) => {
                *self = *val as f32;
                index + 1
            }
            LiveValue::Int64(val) => {
                *self = *val as f32;
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "f32");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Float32(*self)
    }
);

live_primitive!(
    f64,
    0.0f64,
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float32(val) => {
                *self = *val as f64;
                index + 1
            }
            LiveValue::Float64(val) => {
                *self = *val;
                index + 1
            }
            LiveValue::Int64(val) => {
                *self = *val as f64;
                index + 1
            }
            LiveValue::Uint64(val) => {
                *self = *val as f64;
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "f64");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Float64(*self as f64)
    }
);

live_primitive!(
    i64,
    0i64,
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float32(val) => {
                *self = *val as i64;
                index + 1
            }
            LiveValue::Float64(val) => {
                *self = *val as i64;
                index + 1
            }
            LiveValue::Int64(val) => {
                *self = *val as i64;
                index + 1
            }
            LiveValue::Uint64(val) => {
                *self = *val as i64;
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "i64");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Int64(*self)
    }
);

live_primitive!(
    u64,
    0u64,
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float32(val) => {
                *self = *val as u64;
                index + 1
            }
            LiveValue::Float64(val) => {
                *self = *val as u64;
                index + 1
            }
            LiveValue::Int64(val) => {
                *self = *val as u64;
                index + 1
            }
            LiveValue::Uint64(val) => {
                *self = *val as u64;
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "i64");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Uint64(*self)
    }
);

live_primitive!(
    i32,
    0i32,
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float32(val) => {
                *self = *val as i32;
                index + 1
            }
            LiveValue::Float64(val) => {
                *self = *val as i32;
                index + 1
            }
            LiveValue::Int64(val) => {
                *self = *val as i32;
                index + 1
            }
            LiveValue::Uint64(val) => {
                *self = *val as i32;
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "i64");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Int64(*self as i64)
    }
);

live_primitive!(
    u32,
    0u32,
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float32(val) => {
                *self = *val as u32;
                index + 1
            }
            LiveValue::Float64(val) => {
                *self = *val as u32;
                index + 1
            }
            LiveValue::Int64(val) => {
                *self = *val as u32;
                index + 1
            }
            LiveValue::Uint64(val) => {
                *self = *val as u32;
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "i64");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Int64(*self as i64)
    }
);

live_primitive!(
    usize,
    0usize,
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Float32(val) => {
                *self = *val as usize;
                index + 1
            }
            LiveValue::Float64(val) => {
                *self = *val as usize;
                index + 1
            }
            LiveValue::Int64(val) => {
                *self = *val as usize;
                index + 1
            }
            LiveValue::Uint64(val) => {
                *self = *val as usize;
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "i64");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Int64(*self as i64)
    }
);


live_primitive!(
    DVec2,
    DVec2::default(),
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Uint64(v) => {
                *self = DVec2::all(*v as f64);
                index + 1
            }
            LiveValue::Int64(v) => {
                *self = DVec2::all(*v as f64);
                index + 1
            }
            LiveValue::Float32(v) => {
                *self = DVec2::all(*v as f64);
                index + 1
            }
            LiveValue::Float64(v) => {
                *self = DVec2::all(*v);
                index + 1
            }
            LiveValue::Vec2(val) => {
                *self = val.clone().into();
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Vec2");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Vec2(self.clone().into())
    }
);

live_primitive!(
    Vec2,
    Vec2::default(),
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Uint64(v) => {
                *self = Vec2::all(*v as f32);
                index + 1
            }
            LiveValue::Int64(v) => {
                *self = Vec2::all(*v as f32);
                index + 1
            }
            LiveValue::Float32(v) => {
                *self = Vec2::all(*v as f32);
                index + 1
            }
            LiveValue::Float64(v) => {
                *self = Vec2::all(*v as f32);
                index + 1
            }
            LiveValue::Vec2(val) => {
                *self = *val;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Vec2");
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
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Vec2(v) => {
                *self = Vec3{x:v.x, y:v.y, z:0.0};
                index + 1
            }
            LiveValue::Uint64(v) => {
                *self = Vec3::all(*v as f32);
                index + 1
            }            
            LiveValue::Int64(v) => {
                *self = Vec3::all(*v as f32);
                index + 1
            }
            LiveValue::Float32(v) => {
                *self = Vec3::all(*v as f32);
                index + 1
            }
            LiveValue::Float64(v) => {
                *self = Vec3::all(*v as f32);
                index + 1
            }
            LiveValue::Vec3(val) => {
                *self = *val;
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Vec3");
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
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Vec2(v) => {
                *self = Vec4{x:v.x, y:v.y, z:v.x, w:v.y};
                index + 1
            }
            LiveValue::Vec3(v) => {
                *self = Vec4{x:v.x, y:v.y, z:v.z, w:1.0};
                index + 1
            }
            LiveValue::Vec4(v) => {
                *self = Vec4{x:v.x, y:v.y, z:v.z, w:v.w};
                index + 1
            }
            LiveValue::Uint64(v) => {
                *self = Vec4::all(*v as f32);
                index + 1
            }  
            LiveValue::Int64(v) => {
                *self = Vec4::all(*v as f32);
                index + 1
            }
            LiveValue::Float32(v) => {
                *self = Vec4::all(*v as f32);
                index + 1
            }
            LiveValue::Float64(v) => {
                *self = Vec4::all(*v as f32);
                index + 1
            }
            LiveValue::Color(v) => {
                *self = Vec4::from_u32(*v);
                index + 1
            }
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Vec4");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::Color(self.to_u32())
    }
);


live_primitive!(
    Mat4,
    Mat4::default(),
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::DSL {..} => nodes.skip_node(index),
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Vec4");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        LiveValue::None
    }
);

live_primitive!(
    String,
    String::default(),
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Str(v) => {
                self.clear();
                self.push_str(v);
                index + 1
            }
            LiveValue::String(v) => {
                self.clear();
                self.push_str(v.as_str());
                index + 1
            }
            LiveValue::InlineString(v) => {
                self.clear();
                self.push_str(v.as_str());
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::Dependency(path) => {
                match cx.take_dependency(path) {
                    Ok(bytes) => {
                        let string = String::from_utf8_lossy(&bytes);
                        self.push_str(&string);
                        index + 1
                    }
                    Err(_) => {
                        cx.apply_error_resource_not_found(live_error_origin!(), index, nodes, path);
                        nodes.skip_node(index)
                    }
                }
            }
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "String");
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
            LiveValue::String(Arc::new(self.clone()))
        }
    }
);

pub enum ArcStringMut{
    Arc(Arc<String>),
    String(String)
}

impl Default for ArcStringMut{
    fn default()->Self{Self::String(String::new())}
}

impl ArcStringMut{
    pub fn as_arc(&self)->Arc<String>{
        match self{
            Self::Arc(rc)=>{
                return rc.clone();
            }
            Self::String(s)=>{
                return Arc::new(s.clone())
            }
        }
    }
    pub fn as_mut(&mut self)->&mut String{
        match self{
            Self::Arc(rc)=>{
                *self = Self::String(rc.to_string());
                return self.as_mut();
            }
            Self::String(s)=>{
                return s
            }
        }
    }
    pub fn as_mut_empty(&mut self)->&mut String{
        match self{
            Self::Arc(_)=>{
                *self = Self::String(String::new());
                return self.as_mut();
            }
            Self::String(s)=>{
                s.clear();
                return s
            }
        }
    }

    pub fn set(&mut self, v:&str){
        match self{
            Self::Arc(_rc)=>{
                *self = Self::String(v.to_string());
            }
            Self::String(s)=>{
                s.clear();
                s.push_str(v);
            }
        }
    }

    pub fn as_ref(&self)->&str{
        match self{
            Self::Arc(rc)=>{
                &*rc
            }
            Self::String(s)=>{
                return &s
            }
        }
    }
}


live_primitive!(
    ArcStringMut,
    Default::default(),
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Str(v) => {
                *self = ArcStringMut::String(v.to_string());
                index + 1
            }
            LiveValue::String(v) => {
                *self = ArcStringMut::Arc(v.clone());
                index + 1
            }
            LiveValue::InlineString(v) => {
                *self = ArcStringMut::String(v.as_str().to_string());
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            LiveValue::Dependency(path) => {
                match cx.take_dependency(path) {
                    Ok(bytes) => {
                        let string = String::from_utf8_lossy(&bytes);
                        *self = ArcStringMut::String(string.to_string());
                        index + 1
                    }
                    Err(_) => {
                        cx.apply_error_resource_not_found(live_error_origin!(), index, nodes, path);
                        nodes.skip_node(index)
                    }
                }
            }
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "String");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue {
        // lets check our byte size and choose a storage mode appropriately.
        //let bytes = self.as_bytes();
        if let Some(inline_str) = InlineString::from_str(&self.as_ref()) {
            LiveValue::InlineString(inline_str)
        }
        else {
            match self{
                ArcStringMut::Arc(rc)=>LiveValue::String(rc.clone()),
                ArcStringMut::String(v)=>LiveValue::String(Arc::new(v.clone()))
            }
        }
    }
);
/*
live_primitive!(
    Arc<String>,
    Default::default(),
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Str(v) => {
                *self = Arc::new(v.to_string());
                index + 1
            }
            LiveValue::String(v) => {
                *self = v.clone();
                index + 1
            }
            LiveValue::InlineString(v) => {
                *self = Arc::new(v.as_str().to_string());
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Array => {
                if let Some(index) = Animator::last_keyframe_value_from_array(index, nodes) {
                    self.apply(cx, apply, index, nodes);
                }
                nodes.skip_node(index)
            }
            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "String");
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
            LiveValue::String(self.clone())
        }
    }
);*/

impl ToLiveValue for &str{
    fn to_live_value(&self) -> LiveValue {
        // lets check our byte size and choose a storage mode appropriately.
        //let bytes = self.as_bytes();
        if let Some(inline_str) = InlineString::from_str(self) {
            LiveValue::InlineString(inline_str)
        }
        else {
            LiveValue::String(Arc::new(self.to_string()))
        }
    }
}

/*
pub trait LiveIdToEnum{
    fn to_enum(&self) -> LiveValue;
}

impl LiveIdToEnum for &[LiveId;1]{
    fn to_enum(&self) -> LiveValue {
        LiveValue::BareEnum(self[0])
    }
}*/

#[derive(Debug, Default, Clone)]
pub struct LiveDependency(Arc<String>);

impl LiveDependency{
    pub fn as_str(&self)->&str{&self.0}
    pub fn as_ref(&self)->&Arc<String>{&self.0}
}


live_primitive!(
    LiveDependency,
    LiveDependency::default(),
    fn apply(&mut self, cx: &mut Cx, _applyl: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        match &nodes[index].value {
            LiveValue::Dependency (dep)=> {
                *self = Self(dep.clone());
                index + 1
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },

            _ => {
                cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Dependency");
                nodes.skip_node(index)
            }
        }
    },
    fn to_live_value(&self) -> LiveValue { panic!() }
);


live_primitive!(
    LivePtr,
    LivePtr {file_id: LiveFileId(0), index: 0, generation: Default::default()},
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        if let Some(file_id) = apply.from.file_id() {
            *self = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
        }
        nodes.skip_node(index)
    },
    fn to_live_value(&self) -> LiveValue {
        panic!()
    }
);

