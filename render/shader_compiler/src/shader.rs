 
use std::hash::{Hash, Hasher};
use std::any::TypeId;
use crate::ty::*;

#[derive(Clone, Hash, PartialEq)]
pub struct PropInfo{
    pub ident:String,
    pub prop_id:PropId
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShaderSub{
    pub code:String,
    pub instance_props:Vec<PropInfo>,
    pub uniform_props:Vec<PropInfo>
}

#[derive(Default, Clone, PartialEq)]
pub struct ShaderGen {
    pub geometry_vertices: Vec<f32>,
    pub geometry_indices: Vec<u32>,
    pub subs: Vec<ShaderSub>,
}

impl ShaderGen{
    pub fn new() -> Self {
        let sg = ShaderGen::default();
        //let sg = CxShader::def_builtins(sg);
        //let sg = CxShader::def_df(sg);
        //let sg = CxPass::def_uniforms(sg);
        //let sg = CxView::def_uniforms(sg);
        sg
    }
    
    pub fn compose(mut self, sub: ShaderSub) -> Self {
        self.subs.push(sub);
        self
    }
}

impl Hash for ShaderGen {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.geometry_indices.hash(state);
        for vertex in &self.geometry_vertices {
            vertex.to_bits().hash(state);
        }
        self.subs.hash(state);
    }
}

#[derive(Clone, PartialEq, Hash)]
pub enum PropId{
    Float(FloatId),
    Color(ColorId),
    Vec4(Vec4Id),
    Vec3(Vec3Id),
    Vec2(Vec2Id)
}

impl PropId{
    fn shader_ty(&self)->Ty{
        match self{
            PropId::Color(t)=>t.shader_ty(),
            PropId::Vec4(t)=>t.shader_ty(),
            PropId::Vec3(t)=>t.shader_ty(),
            PropId::Vec2(t)=>t.shader_ty(),
            PropId::Float(t)=>t.shader_ty(),
        }
    }
}

#[derive(Hash, PartialEq, Copy, Clone)]
pub struct ColorId(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone)]
pub struct Vec4Id(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone)]
pub struct Vec3Id(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone)]
pub struct Vec2Id(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone)]
pub struct FloatId(pub TypeId);


#[macro_export]
macro_rules!uid { 
    () => {{
        struct Unique{};
        std::any::TypeId::of::<Unique>().into()
    }}
}

impl ColorId{
    pub fn shader_ty(&self) -> Ty{Ty::Vec4}
    pub fn prop_id(&self) -> PropId{PropId::Color(*self)}
}

impl Into<ColorId> for TypeId{
    fn into(self) -> ColorId{ColorId(self)}
}


impl Vec4Id{
    pub fn shader_ty(&self) -> Ty{Ty::Vec4}
    pub fn prop_id(&self) -> PropId{PropId::Vec4(*self)}
}

impl Into<Vec4Id> for TypeId{
    fn into(self) -> Vec4Id{Vec4Id(self)}
}

impl Vec3Id{
    pub fn shader_ty(&self) -> Ty{Ty::Vec3}
    pub fn prop_id(&self) -> PropId{PropId::Vec3(*self)}
}

impl Into<Vec3Id> for TypeId{
    fn into(self) -> Vec3Id{Vec3Id(self)}
}

impl Vec2Id{
    pub fn shader_ty(&self) -> Ty{Ty::Vec2}
    pub fn prop_id(&self) -> PropId{PropId::Vec2(*self)}
}

impl Into<Vec2Id> for TypeId{
    fn into(self) -> Vec2Id{Vec2Id(self)}
}

impl FloatId{
    pub fn shader_ty(&self) -> Ty{Ty::Float}
    pub fn prop_id(&self) -> PropId{PropId::Float(*self)}
}

impl Into<FloatId> for TypeId{
    fn into(self) -> FloatId{FloatId(self)}
}