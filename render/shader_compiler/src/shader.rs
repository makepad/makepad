
use std::hash::{Hash, Hasher};
use std::any::TypeId;
use crate::ty::*;

#[derive(Clone, Copy, Hash, PartialEq, Debug)]
pub struct LiveLoc{
    pub file:&'static str,
    pub line:usize,
    pub column:usize
}


#[derive(Clone, Hash, PartialEq)]
pub struct PropDef{
    pub name: String,
    pub ident: String,
    pub prop_id:PropId
}

#[derive(Clone, Hash, PartialEq)]
pub struct ShaderSub{
    pub loc:LiveLoc,
    pub code:String,
    pub instance_props:Vec<PropDef>,
    pub uniform_props:Vec<PropDef>
}

#[derive(Default, Clone, PartialEq)]
pub struct ShaderGen {
    pub geometry_vertices: Vec<f32>,
    pub geometry_indices: Vec<u32>,
    pub subs: Vec<ShaderSub>,
}

impl Eq for ShaderGen {}

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
    Texture2d(Texture2dId),
    Float(FloatId),
    Color(ColorId),
    Vec4(Vec4Id),
    Vec3(Vec3Id),
    Vec2(Vec2Id)
}

impl PropId{
    pub fn shader_ty(&self)->Ty{
        match self{
            PropId::Texture2d(t)=>t.shader_ty(),
            PropId::Color(t)=>t.shader_ty(),
            PropId::Vec4(t)=>t.shader_ty(),
            PropId::Vec3(t)=>t.shader_ty(),
            PropId::Vec2(t)=>t.shader_ty(),
            PropId::Float(t)=>t.shader_ty(),
        }
    }
}

#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Texture2dId(pub TypeId);

impl Into<PropId> for Texture2dId{
    fn into(self) -> PropId{PropId::Texture2d(self)}
}

#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct ColorId(pub TypeId);

impl Into<PropId> for ColorId{
    fn into(self) -> PropId{PropId::Color(self)}
}

#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec4Id(pub TypeId);

impl Into<PropId> for Vec4Id{
    fn into(self) -> PropId{PropId::Vec4(self)}
}

#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec3Id(pub TypeId);

impl Into<PropId> for Vec3Id{
    fn into(self) -> PropId{PropId::Vec3(self)}
}


#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec2Id(pub TypeId);

impl Into<PropId> for Vec2Id{
    fn into(self) -> PropId{PropId::Vec2(self)}
}


#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct FloatId(pub TypeId);

impl Into<PropId> for FloatId{
    fn into(self) -> PropId{PropId::Float(self)}
}


#[macro_export]
macro_rules!uid { 
    () => {{
        struct Unique{};
        std::any::TypeId::of::<Unique>().into()
    }}
}

impl Texture2dId{
    pub fn shader_ty(&self) -> Ty{Ty::Texture2d}
    pub fn prop_id(&self) -> PropId{PropId::Texture2d(*self)}
}

impl Into<Texture2dId> for TypeId{
    fn into(self) -> Texture2dId{Texture2dId(self)}
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
