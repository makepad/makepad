use crate::ident::Ident;
use std::fmt;
use std::rc::Rc;
use crate::span::{Span};
use std::cell::{RefCell};

#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum Ty {
    Void,
    Bool,
    Int,
    Float,
    Bvec2,
    Bvec3,
    Bvec4,
    Ivec2,
    Ivec3,
    Ivec4,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
    Texture2D,
    Array { elem_ty: Rc<Ty>, len: usize },
    Struct { ident: Ident },
}

impl Ty {
    pub fn maybe_ty_lit(&self)->Option<TyLit>{
        match self{
            Ty::Void => None,
            Ty::Bool => Some(TyLit::Bool),
            Ty::Int =>  Some(TyLit::Int),
            Ty::Float => Some(TyLit::Float),
            Ty::Bvec2 => Some(TyLit::Bvec2),
            Ty::Bvec3 => Some(TyLit::Bvec3),
            Ty::Bvec4 => Some(TyLit::Bvec4),
            Ty::Ivec2 => Some(TyLit::Ivec2),
            Ty::Ivec3 => Some(TyLit::Ivec3),
            Ty::Ivec4 => Some(TyLit::Ivec4),
            Ty::Vec2 => Some(TyLit::Vec2),
            Ty::Vec3 => Some(TyLit::Vec3),
            Ty::Vec4 => Some(TyLit::Vec4),
            Ty::Mat2 => Some(TyLit::Mat2),
            Ty::Mat3 => Some(TyLit::Mat3),
            Ty::Mat4 => Some(TyLit::Mat4),
            Ty::Texture2D => Some(TyLit::Bool),
            Ty::Array { .. } => None,
            Ty::Struct { .. } => None
        }
    }
    
    pub fn is_scalar(&self) -> bool {
        match self {
            Ty::Bool | Ty::Int | Ty::Float => true,
            _ => false,
        }
    }

    pub fn is_vector(&self) -> bool {
        match self {
            Ty::Bvec2
            | Ty::Bvec3
            | Ty::Bvec4
            | Ty::Ivec2
            | Ty::Ivec3
            | Ty::Ivec4
            | Ty::Vec2
            | Ty::Vec3
            | Ty::Vec4 => true,
            _ => false,
        }
    }

    pub fn is_matrix(&self) -> bool {
        match self {
            Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => true,
            _ => false,
        }
    }

    pub fn size(&self) -> usize {
        match self {
            Ty::Void => 0,
            Ty::Bool | Ty::Int | Ty::Float => 1,
            Ty::Bvec2 | Ty::Ivec2 | Ty::Vec2 => 2,
            Ty::Bvec3 | Ty::Ivec3 | Ty::Vec3 => 3,
            Ty::Bvec4 | Ty::Ivec4 | Ty::Vec4 | Ty::Mat2 => 4,
            Ty::Mat3 => 9,
            Ty::Mat4 => 16,
            Ty::Texture2D { .. } => panic!(),
            Ty::Array { elem_ty, len } => elem_ty.size() * len,
            Ty::Struct { .. } => panic!(),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Ty::Void => write!(f, "void"),
            Ty::Bool => write!(f, "bool"),
            Ty::Int => write!(f, "int"),
            Ty::Float => write!(f, "float"),
            Ty::Bvec2 => write!(f, "bvec2"),
            Ty::Bvec3 => write!(f, "bvec3"),
            Ty::Bvec4 => write!(f, "bvec4"),
            Ty::Ivec2 => write!(f, "ivec2"),
            Ty::Ivec3 => write!(f, "ivec3"),
            Ty::Ivec4 => write!(f, "ivec4"),
            Ty::Vec2 => write!(f, "vec2"),
            Ty::Vec3 => write!(f, "vec3"),
            Ty::Vec4 => write!(f, "vec4"),
            Ty::Mat2 => write!(f, "mat2"),
            Ty::Mat3 => write!(f, "mat3"),
            Ty::Mat4 => write!(f, "mat4"),
            Ty::Texture2D => write!(f, "texture2D"),
            Ty::Array { elem_ty, len } => write!(f, "{}[{}]", elem_ty, len),
            Ty::Struct { ident, .. } => write!(f, "{}", ident),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TyExpr {
    pub ty: RefCell<Option<Ty >>,
    pub kind: TyExprKind,
}

#[derive(Clone, Debug)]
pub enum TyExprKind {
    Array {
        span: Span,
        elem_ty_expr: Box<TyExpr>,
        len: u32,
    },
    Var {
        span: Span,
        ident: Ident,
    },
    Lit {
        span: Span,
        ty_lit: TyLit,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum TyLit {
    Bool,
    Int,
    Float,
    Bvec2,
    Bvec3,
    Bvec4,
    Ivec2,
    Ivec3,
    Ivec4,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
    Texture2D,
}

impl TyLit {
    pub fn to_ty_expr(self) ->TyExpr{
        TyExpr {
            ty: RefCell::new(None),
            kind: TyExprKind::Lit {
                span: Span::default(),
                ty_lit: self
            }
        }
    }
    
    pub fn to_ty(self) -> Ty {
        match self {
            TyLit::Bool => Ty::Bool,
            TyLit::Int => Ty::Int,
            TyLit::Float => Ty::Float,
            TyLit::Bvec2 => Ty::Bvec2,
            TyLit::Bvec3 => Ty::Bvec3,
            TyLit::Bvec4 => Ty::Bvec4,
            TyLit::Ivec2 => Ty::Ivec2,
            TyLit::Ivec3 => Ty::Ivec3,
            TyLit::Ivec4 => Ty::Ivec4,
            TyLit::Vec2 => Ty::Vec2,
            TyLit::Vec3 => Ty::Vec3,
            TyLit::Vec4 => Ty::Vec4,
            TyLit::Mat2 => Ty::Mat2,
            TyLit::Mat3 => Ty::Mat3,
            TyLit::Mat4 => Ty::Mat4,
            TyLit::Texture2D => Ty::Texture2D,
        }
    }
    
    pub fn from_rust_type_str(rust_type:&str)->Option<TyLit>{
        let ident = Ident::new(rust_type);
        if ident == Ident::new("bool"){return Some(TyLit::Bool)}
        if ident == Ident::new("i32"){return Some(TyLit::Int)}
        if ident == Ident::new("f32"){return Some(TyLit::Float)}
        if ident == Ident::new("Vec2"){return Some(TyLit::Vec2)}
        if ident == Ident::new("Vec3"){return Some(TyLit::Vec3)}
        if ident == Ident::new("Vec4"){return Some(TyLit::Vec4)}
        if ident == Ident::new("Mat2"){return Some(TyLit::Mat2)}
        if ident == Ident::new("Mat3"){return Some(TyLit::Mat3)}
        if ident == Ident::new("Mat4"){return Some(TyLit::Mat4)}
        if ident == Ident::new("Texture2D"){return Some(TyLit::Texture2D)}
        if ident == Ident::new("BVec2"){return Some(TyLit::Bvec2)}
        if ident == Ident::new("BVec3"){return Some(TyLit::Bvec3)}
        if ident == Ident::new("BVec4"){return Some(TyLit::Bvec4)}
        if ident == Ident::new("IVec2"){return Some(TyLit::Ivec2)}
        if ident == Ident::new("IVec3"){return Some(TyLit::Ivec3)}
        if ident == Ident::new("IVec4"){return Some(TyLit::Ivec4)}
        None
    }
}

impl fmt::Display for TyLit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                TyLit::Bool => "bool",
                TyLit::Int => "int",
                TyLit::Float => "float",
                TyLit::Bvec2 => "bvec2",
                TyLit::Bvec3 => "bvec3",
                TyLit::Bvec4 => "bvec4",
                TyLit::Ivec2 => "ivec2",
                TyLit::Ivec3 => "ivec3",
                TyLit::Ivec4 => "ivec4",
                TyLit::Vec2 => "vec2",
                TyLit::Vec3 => "vec3",
                TyLit::Vec4 => "vec4",
                TyLit::Mat2 => "mat2",
                TyLit::Mat3 => "mat3",
                TyLit::Mat4 => "mat4",
                TyLit::Texture2D => "texture2D",
            }
        )
    }
}


