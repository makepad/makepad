use crate::analyse;
use crate::ast::ShaderAst;
use crate::error::Error;
use crate::lex;
use crate::parse;
use crate::ty::*;
use std::any::TypeId;
use std::fmt;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Hash, PartialEq, Debug)]
pub struct LiveLoc {
    pub path: &'static str,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct PropDef {
    pub name: String,
    pub ident: String,
    pub block: Option<String>,
    pub prop_id: PropId,
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct ShaderSub {
    pub loc: LiveLoc,
    pub code: String,
    pub geometries: Vec<PropDef>,
    pub instances: Vec<PropDef>,
    pub uniforms: Vec<PropDef>,
    pub textures: Vec<PropDef>,
}

#[derive(Default, Clone, PartialEq)]
pub struct ShaderGen {
    pub geometry_vertices: Vec<f32>,
    pub geometry_indices: Vec<u32>,
    pub subs: Vec<ShaderSub>,
}

impl Eq for ShaderGen {}

#[derive(Clone, Debug, PartialEq)]
pub struct ShaderGenError {
    pub path: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub msg: String,
}

impl fmt::Display for ShaderGenError {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}: {} {} - {}",
            self.path, self.line, self.col, self.msg
        )
    }
}

impl ShaderGen {
    pub fn new() -> Self {
        ShaderGen::default()
    }

    pub fn byte_to_row_col(byte: usize, source: &str) -> (usize, usize) {
        let lines = source.split("\n");
        let mut o = 0;
        for (index, line) in lines.enumerate() {
            if byte >= o && byte < o + line.len() {
                return (index, byte - o);
            }
            o += line.len() + 1;
        }
        return (0, 0);
    }

    pub fn shader_gen_error(err: &Error, sub: &ShaderSub) -> ShaderGenError {
        // lets find the span info
        let start = ShaderGen::byte_to_row_col(err.span.start, &sub.code);
        ShaderGenError {
            path: sub.loc.path.to_string(),
            line: start.0 + sub.loc.line,
            col: start.1 + 1,
            len: err.span.end - err.span.start,
            msg: err.to_string(),
        }
    }

    pub fn compose(mut self, sub: ShaderSub) -> Self {
        self.subs.push(sub);
        self
    }

    pub fn lex_parse_analyse(&self) -> Result<ShaderAst, ShaderGenError> {
        let mut shader_ast = ShaderAst::new();
        let mut inputs = Vec::new();
        for (index, sub) in self.subs.iter().enumerate() {
            // lets tokenize the sub
            let tokens = lex::lex(sub.code.chars(), index).collect::<Result<Vec<_>, _>>();
            if let Err(err) = &tokens {
                return Err(Self::shader_gen_error(err, sub));
            }
            let tokens = tokens.unwrap();
            if let Err(err) = parse::parse(&tokens, &mut shader_ast) {
                return Err(Self::shader_gen_error(&err, sub));
            }
            // lets add our instance_props
            inputs.extend(sub.geometries.iter());
            inputs.extend(sub.instances.iter());
            inputs.extend(sub.uniforms.iter());
            inputs.extend(sub.textures.iter());
        }

        // lets collect all our
        // ok now we have the shader, lets analyse
        if let Err(err) = analyse::analyse(&mut shader_ast, &inputs) {
            let sub = &self.subs[err.span.loc_id];
            return Err(Self::shader_gen_error(&err, sub));
        }

        Ok(shader_ast)
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

#[derive(Debug, Clone, PartialEq, Hash)]
pub enum PropId {
    Texture2d(Texture2dId),
    Color(ColorId),
    Vec4(Vec4Id),
    Vec3(Vec3Id),
    Vec2(Vec2Id),
    Float(FloatId),
    Mat4(Mat4Id),
}

impl PropId {
    pub fn shader_ty(&self) -> Ty {
        match self.clone() {
            PropId::Texture2d(t) => t.into(),
            PropId::Color(t) => t.into(),
            PropId::Vec4(t) => t.into(),
            PropId::Vec3(t) => t.into(),
            PropId::Vec2(t) => t.into(),
            PropId::Float(t) => t.into(),
            PropId::Mat4(t) => t.into(),
        }
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Texture2dId(pub TypeId);

impl Into<PropId> for Texture2dId {
    fn into(self) -> PropId {
        PropId::Texture2d(self)
    }
}

impl Into<Ty> for Texture2dId {
    fn into(self) -> Ty {
        Ty::Texture2D
    }
}

impl Into<Texture2dId> for TypeId {
    fn into(self) -> Texture2dId {
        Texture2dId(self)
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct ColorId(pub TypeId);

impl Into<PropId> for ColorId {
    fn into(self) -> PropId {
        PropId::Color(self)
    }
}

impl Into<Ty> for ColorId {
    fn into(self) -> Ty {
        Ty::Vec4
    }
}

impl Into<ColorId> for TypeId {
    fn into(self) -> ColorId {
        ColorId(self)
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec4Id(pub TypeId);

impl Into<PropId> for Vec4Id {
    fn into(self) -> PropId {
        PropId::Vec4(self)
    }
}

impl Into<Vec4Id> for TypeId {
    fn into(self) -> Vec4Id {
        Vec4Id(self)
    }
}

impl Into<Ty> for Vec4Id {
    fn into(self) -> Ty {
        Ty::Vec4
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec3Id(pub TypeId);

impl Into<PropId> for Vec3Id {
    fn into(self) -> PropId {
        PropId::Vec3(self)
    }
}

impl Into<Vec3Id> for TypeId {
    fn into(self) -> Vec3Id {
        Vec3Id(self)
    }
}

impl Into<Ty> for Vec3Id {
    fn into(self) -> Ty {
        Ty::Vec3
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Vec2Id(pub TypeId);

impl Into<PropId> for Vec2Id {
    fn into(self) -> PropId {
        PropId::Vec2(self)
    }
}

impl Into<Vec2Id> for TypeId {
    fn into(self) -> Vec2Id {
        Vec2Id(self)
    }
}

impl Into<Ty> for Vec2Id {
    fn into(self) -> Ty {
        Ty::Vec2
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct FloatId(pub TypeId);

impl Into<PropId> for FloatId {
    fn into(self) -> PropId {
        PropId::Float(self)
    }
}

impl Into<FloatId> for TypeId {
    fn into(self) -> FloatId {
        FloatId(self)
    }
}

impl Into<Ty> for FloatId {
    fn into(self) -> Ty {
        Ty::Float
    }
}

#[derive(Debug, Hash, PartialEq, Copy, Clone, Eq)]
pub struct Mat4Id(pub TypeId);

impl Into<PropId> for Mat4Id {
    fn into(self) -> PropId {
        PropId::Mat4(self)
    }
}

impl Into<Mat4Id> for TypeId {
    fn into(self) -> Mat4Id {
        Mat4Id(self)
    }
}

impl Into<Ty> for Mat4Id {
    fn into(self) -> Ty {
        Ty::Mat4
    }
}

#[macro_export]
macro_rules! uid {
    () => {{
        struct Unique {};
        std::any::TypeId::of::<Unique>().into()
    }};
}
