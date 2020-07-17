
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
    pub attribute_props:Vec<PropDef>,
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
        ShaderGen::default()
    }
        
        
    pub fn byte_to_row_col(byte:usize, source:&str)->(usize,usize){
        let lines = source.split("\n");
        let mut o = 0;
        for (index,line) in lines.enumerate(){
            if byte >= o && byte < o+line.len(){
                return (index, byte - o)
            }
            o += line.len() + 1;
        }
        return (0,0)
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
    Color(ColorId),
    Vec4(Vec4Id),
    Vec3(Vec3Id),
    Vec2(Vec2Id),
    Float(FloatId),
    Mat4(Mat4Id)
}

impl PropId{
    pub fn shader_ty(&self)->Ty{
        match self.clone(){
            PropId::Texture2d(t)=>t.into(),
            PropId::Color(t)=>t.into(),
            PropId::Vec4(t)=>t.into(),
            PropId::Vec3(t)=>t.into(),
            PropId::Vec2(t)=>t.into(),
            PropId::Float(t)=>t.into(),
            PropId::Mat4(t)=>t.into(),
        }
    }
}

#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Texture2dId(pub TypeId);

impl Into<PropId> for Texture2dId{
    fn into(self) -> PropId{PropId::Texture2d(self)}
}


impl Into<Ty> for Texture2dId{
    fn into(self) -> Ty{Ty::Texture2d}
}


impl Into<Texture2dId> for TypeId{
    fn into(self) -> Texture2dId{Texture2dId(self)}
}


#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct ColorId(pub TypeId);

impl Into<PropId> for ColorId{
    fn into(self) -> PropId{PropId::Color(self)}
}

impl Into<Ty> for ColorId{
    fn into(self) -> Ty{Ty::Vec4}
}

impl Into<ColorId> for TypeId{
    fn into(self) -> ColorId{ColorId(self)}
}




#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec4Id(pub TypeId);

impl Into<PropId> for Vec4Id{
    fn into(self) -> PropId{PropId::Vec4(self)}
}

impl Into<Vec4Id> for TypeId{
    fn into(self) -> Vec4Id{Vec4Id(self)}
}

impl Into<Ty> for Vec4Id{
    fn into(self) -> Ty{Ty::Vec4}
}



#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec3Id(pub TypeId);

impl Into<PropId> for Vec3Id{
    fn into(self) -> PropId{PropId::Vec3(self)}
}

impl Into<Vec3Id> for TypeId{
    fn into(self) -> Vec3Id{Vec3Id(self)}
}

impl Into<Ty> for Vec3Id{
    fn into(self) -> Ty{Ty::Vec3}
}



#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec2Id(pub TypeId);

impl Into<PropId> for Vec2Id{
    fn into(self) -> PropId{PropId::Vec2(self)}
}

impl Into<Vec2Id> for TypeId{
    fn into(self) -> Vec2Id{Vec2Id(self)}
}

impl Into<Ty> for Vec2Id{
    fn into(self) -> Ty{Ty::Vec2}
}



#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct FloatId(pub TypeId);

impl Into<PropId> for FloatId{
    fn into(self) -> PropId{PropId::Float(self)}
}

impl Into<FloatId> for TypeId{
    fn into(self) -> FloatId{FloatId(self)}
}

impl Into<Ty> for FloatId{
    fn into(self) -> Ty{Ty::Float}
}


#[derive(Hash, PartialEq, Copy, Clone, Eq)]
pub struct Mat4Id(pub TypeId);

impl Into<PropId> for Mat4Id{
    fn into(self) -> PropId{PropId::Mat4(self)}
}

impl Into<Mat4Id> for TypeId{
    fn into(self) -> Mat4Id{Mat4Id(self)}
}

impl Into<Ty> for Mat4Id{
    fn into(self) -> Ty{Ty::Mat4}
}



#[macro_export]
macro_rules!uid { 
    () => {{
        struct Unique{};
        std::any::TypeId::of::<Unique>().into()
    }}
}