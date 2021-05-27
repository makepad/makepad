use makepad_live_parser::Span;
use makepad_live_parser::Token;
use makepad_live_parser::Id;
use makepad_live_parser::token_punct;
use makepad_live_parser::FullNodePtr;
use makepad_live_parser::Vec4;
use std::fmt::Write;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fmt;
use std::rc::Rc;
use makepad_live_parser::PrettyPrintedF32;
use makepad_live_parser::id;
use crate::shaderregistry::ShaderResourceId;

// all the Live node pointer newtypes

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct FnNodePtr(pub FullNodePtr);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct StructNodePtr(pub FullNodePtr);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ShaderNodePtr(pub FullNodePtr);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ConstNodePtr(pub FullNodePtr);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct ValueNodePtr(pub FullNodePtr);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct VarDefNodePtr(pub FullNodePtr);

//impl FnNodePtr {pub fn to_scope_ptr(self) -> ScopeNodePtr {ScopeNodePtr(self.0)}}
//impl VarDefNodePtr {pub fn to_scope_ptr(self) -> ScopeNodePtr {ScopeNodePtr(self.0)}}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum InputNodePtr {
    VarDef(FullNodePtr),
    Class(FullNodePtr)
}


#[derive(Clone, Debug, Default)]
pub struct DrawShaderDecl {
    pub debug: bool,
    pub default_geometry: Option<ShaderResourceId>,
    pub decls: Vec<Decl>,
    pub methods: Vec<FnDecl>,
}


#[derive(Clone, Debug)]
pub enum Decl {
    Geometry(GeometryDecl),
    Instance(InstanceDecl),
    Texture(TextureDecl),
    Uniform(UniformDecl),
    Varying(VaryingDecl),
}

#[derive(Clone, Debug)]
pub struct GeometryDecl {
    pub is_used_in_fragment_shader: Cell<Option<bool >>,
    pub var_def_node_ptr: VarDefNodePtr,
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}
/*
#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub expr: Expr,
}*/

// the unique identification of a fn call
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum Callee {
    PlainFn{fn_node_ptr:FnNodePtr},
    ShaderMethod{shader_node_ptr:ShaderNodePtr, ident:Ident},
    StructMethod{struct_node_ptr:StructNodePtr, ident:Ident},
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub fn_node_ptr: FnNodePtr,
    
    pub ident: Ident,

    pub self_kind: Option<TyExprKind>, 
    
    // analysis
    pub callees: RefCell<Option<BTreeSet<Callee >> >,
    pub builtin_deps: RefCell<Option<BTreeSet<Ident >> >,

    // which props we reffed on self
    pub self_refs: RefCell<Option<BTreeSet<Ident >> >,

    pub cons_fn_deps: RefCell<Option<BTreeSet<(TyLit, Vec<Ty>) >> >,

    // base
    pub span: Span,
    pub return_ty: RefCell<Option<Ty >>,
    pub params: Vec<Param>,
    pub return_ty_expr: Option<TyExpr>,
    pub block: Block,
    pub first_param_is_self: bool
}


#[derive(Clone, Debug)]
pub struct StructDecl {
    pub span: Span,
    //pub ident: Ident,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<FnDecl>, 
}

impl StructDecl {
    pub fn find_field(&self, ident: Ident) -> Option<&FieldDecl> {
        self.fields.iter().find( | field | field.ident == ident)
    }
}
#[derive(Clone, Debug)]
pub struct InstanceDecl {
    pub is_used_in_fragment_shader: Cell<Option<bool >>,
    pub input_node_ptr: InputNodePtr,
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct TextureDecl {
    pub input_node_ptr: InputNodePtr,
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct UniformDecl {
    pub input_node_ptr: InputNodePtr,
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub block_ident: Ident,
}

#[derive(Clone, Debug)]
pub struct VaryingDecl {
    pub var_def_node_ptr: VarDefNodePtr,
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub span: Span,
    pub is_inout: bool,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub var_def_node_ptr: VarDefNodePtr,
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
    For {
        span: Span,
        ident: Ident,
        from_expr: Expr,
        to_expr: Expr,
        step_expr: Option<Expr>,
        block: Box<Block>,
    },
    If {
        span: Span,
        expr: Expr,
        block_if_true: Box<Block>,
        block_if_false: Option<Box<Block >>,
    },
    Let {
        span: Span,
        ty: RefCell<Option<Ty >>,
        ident: Ident,
        ty_expr: Option<TyExpr>,
        expr: Option<Expr>,
    },
    Return {
        span: Span,
        expr: Option<Expr>,
    },
    Block {
        span: Span,
        block: Box<Block>,
    },
    Expr {
        span: Span,
        expr: Expr,
    },
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub span: Span,
    pub ty: RefCell<Option<Ty >>,
    pub const_val: RefCell<Option<Option<Val >> >,
    pub const_index: Cell<Option<usize >>,
    pub kind: ExprKind,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Cond {
        span: Span,
        expr: Box<Expr>,
        expr_if_true: Box<Expr>,
        expr_if_false: Box<Expr>,
    },
    Bin {
        span: Span,
        op: BinOp,
        left_expr: Box<Expr>,
        right_expr: Box<Expr>,
    },
    Un {
        span: Span,
        op: UnOp,
        expr: Box<Expr>,
    },
    Field {
        span: Span,
        expr: Box<Expr>,
        field_ident: Ident,
    },
    Index {
        span: Span,
        expr: Box<Expr>,
        index_expr: Box<Expr>,
    },
    MethodCall {
        span: Span,
        ident: Ident,
        arg_exprs: Vec<Expr>,
    },
    PlainCall {
        span: Span,
        fn_node_ptr: FnNodePtr,
        arg_exprs: Vec<Expr>,
    },
    BuiltinCall{
        span: Span,
        ident: Ident,
        arg_exprs: Vec<Expr>,
    },
    ConsCall {
        span: Span,
        ty_lit: TyLit,
        arg_exprs: Vec<Expr>,
    },
    Var {
        span: Span,
        kind: Cell<Option<VarKind >>,
        ident_path: IdentPath,
    },
    Lit {
        span: Span,
        lit: Lit,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum VarKind {
    Local,
    MutLocal,
    Const(ConstNodePtr),
    LiveValue(ValueNodePtr)
}

#[derive(Clone, Debug)]
pub struct TyExpr {
    pub span: Span,
    pub ty: RefCell<Option<Ty >>,
    pub kind: TyExprKind,
}

#[derive(Clone, Debug)]
pub enum TyExprKind {
    Array {
        elem_ty_expr: Box<TyExpr>,
        len: u32,
    },
    Struct(StructNodePtr),
    Shader(ShaderNodePtr),
    Lit {
        ty_lit: TyLit,
    },
}


#[derive(Clone, Copy, Debug)]
pub enum MacroCallAnalysis {
}

#[derive(Clone, Copy, Debug)]
pub enum BinOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
}



#[derive(Clone, Copy, Debug)]
pub enum UnOp {
    Not,
    Neg,
}

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
    Array {elem_ty: Rc<Ty>, len: usize},
    Struct(StructNodePtr),
    Shader(ShaderNodePtr),
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lit {
    Bool(bool),
    Int(i32),
    Float(f32),
    Color(u32),
}



#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Bool(bool),
    Int(i32),
    Float(f32),
    Vec4(Vec4),
}


#[derive(Clone, Copy, Ord, PartialOrd, Default, Eq, Hash, PartialEq)]
pub struct Ident(pub Id);

/*
impl ShaderBody {
    
    pub fn find_const_decl(&self, _ident: Ident) -> Option<&ConstDecl> {
        /*
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Const(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
        */
        return None
    }
    
    pub fn find_fn_decl(&self, ident: Ident) -> Option<&FnDecl> {
        self.fn_decls.iter().rev().find_map( | decl | {
            Some(decl)
                .filter( | decl | decl.ident == ident)
        })
    }
}*/

impl FnDecl{
    pub fn init_analysis(&self){
        *self.callees.borrow_mut() = Some(BTreeSet::new());
        *self.builtin_deps.borrow_mut() = Some(BTreeSet::new());
        *self.cons_fn_deps.borrow_mut() = Some(BTreeSet::new());
        *self.self_refs.borrow_mut() = Some(BTreeSet::new());
    }
}

impl DrawShaderDecl {
    
    pub fn find_geometry_decl(&self, ident: Ident) -> Option<&GeometryDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Geometry(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
    
    /*
    pub fn find_fn_decl(&self, ident_path: IdentPath) -> Option<&FnDecl> {
        self.shader_body.find_fn_
        self.fn_decls.iter().rev().find_map( | decl | {
            Some(decl)
            .filter( | decl | decl.ident_path == ident_path)
        })
    }*/
    
    pub fn find_instance_decl(&self, ident: Ident) -> Option<&InstanceDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Instance(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
    /*
    pub fn find_struct_decl(&self, ident: Ident) -> Option<&StructDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Struct(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }*/
    
    pub fn find_uniform_decl(&self, ident: Ident) -> Option<&UniformDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Uniform(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
    
    pub fn find_varying_decl(&self, ident: Ident) -> Option<&VaryingDecl> {
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Varying(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
    }
}

impl BinOp {
    pub fn from_assign_op(token: Token) -> Option<BinOp> {
        match token {
            token_punct!( =) => Some(BinOp::Assign),
            token_punct!( +=) => Some(BinOp::AddAssign),
            token_punct!( -=) => Some(BinOp::SubAssign),
            token_punct!( *=) => Some(BinOp::MulAssign),
            token_punct!( /=) => Some(BinOp::DivAssign),
            _ => None,
        }
    }
    
    pub fn from_or_op(token: Token) -> Option<BinOp> {
        match token {
            token_punct!( ||) => Some(BinOp::Or),
            _ => None,
        }
    }
    
    pub fn from_and_op(token: Token) -> Option<BinOp> {
        match token {
            token_punct!( &&) => Some(BinOp::And),
            _ => None,
        }
    }
    
    pub fn from_eq_op(token: Token) -> Option<BinOp> {
        match token {
            token_punct!( ==) => Some(BinOp::Eq),
            token_punct!( !=) => Some(BinOp::Ne),
            _ => None,
        }
    }
    
    pub fn from_rel_op(token: Token) -> Option<BinOp> {
        match token {
            token_punct!(<) => Some(BinOp::Lt),
            token_punct!( <=) => Some(BinOp::Le),
            token_punct!(>) => Some(BinOp::Gt),
            token_punct!( >=) => Some(BinOp::Ge),
            _ => None,
        }
    }
    
    pub fn from_add_op(token: Token) -> Option<BinOp> {
        match token {
            token_punct!( +) => Some(BinOp::Add),
            token_punct!(-) => Some(BinOp::Sub),
            _ => None,
        }
    }
    
    pub fn from_mul_op(token: Token) -> Option<BinOp> {
        match token {
            token_punct!(*) => Some(BinOp::Mul),
            token_punct!( /) => Some(BinOp::Div),
            _ => None,
        }
    }
    
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                BinOp::Assign => "=",
                BinOp::AddAssign => "+=",
                BinOp::SubAssign => "-=",
                BinOp::MulAssign => "*=",
                BinOp::DivAssign => "/=",
                BinOp::Or => "||",
                BinOp::And => "&&",
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
            }
        )
    }
}

impl UnOp {
    pub fn from_un_op(token: Token) -> Option<UnOp> {
        match token {
            token_punct!(!) => Some(UnOp::Not),
            token_punct!(-) => Some(UnOp::Neg),
            _ => None,
        }
    }
}

impl fmt::Display for UnOp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                UnOp::Not => "!",
                UnOp::Neg => "-",
            }
        )
    }
}


impl Ty {
    
    pub fn maybe_ty_lit(&self) -> Option<TyLit> {
        match self {
            Ty::Void => None,
            Ty::Bool => Some(TyLit::Bool),
            Ty::Int => Some(TyLit::Int),
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
            Ty::Array {..} => None,
            Ty::Struct(_) => None,
            Ty::Shader(_) => None
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
            Ty::Texture2D {..} => panic!(),
            Ty::Array {elem_ty, len} => elem_ty.size() * len,
            Ty::Struct(_) => panic!(),
            Ty::Shader(_) =>  panic!(),
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
            Ty::Array {elem_ty, len} => write!(f, "{}[{}]", elem_ty, len),
            Ty::Struct(struct_ptr)=> write!(f, "Struct:{:?}", struct_ptr),
            Ty::Shader(shader_ptr) =>  write!(f, "Shader:{:?}", shader_ptr),
        }
    }
}

impl TyLit {
    pub fn from_id(id: Id) -> Option<TyLit> {
        match id {
            id!(vec4) => Some(TyLit::Vec4),
            id!(vec3) => Some(TyLit::Vec3),
            id!(vec2) => Some(TyLit::Vec2),
            id!(mat4) => Some(TyLit::Mat4),
            id!(mat3) => Some(TyLit::Mat3),
            id!(mat2) => Some(TyLit::Mat2),
            id!(float) => Some(TyLit::Float),
            id!(bool) => Some(TyLit::Bool),
            id!(int) => Some(TyLit::Int),
            id!(bvec2) => Some(TyLit::Bvec2),
            id!(bvec3) => Some(TyLit::Bvec3),
            id!(bvec4) => Some(TyLit::Bvec4),
            id!(ivec2) => Some(TyLit::Ivec4),
            id!(ivec3) => Some(TyLit::Ivec4),
            id!(ivec4) => Some(TyLit::Ivec4),
            id!(texture2D) => Some(TyLit::Texture2D),
            _ => None
        }
    }
    
    pub fn to_ty_expr(self) -> TyExpr {
        TyExpr {
            ty: RefCell::new(None),
            span: Span::default(),
            kind: TyExprKind::Lit {
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


impl Lit {
    pub fn to_ty(self) -> Ty {
        match self {
            Lit::Bool(_) => Ty::Bool,
            Lit::Int(_) => Ty::Int,
            Lit::Float(_) => Ty::Float,
            Lit::Color(_) => Ty::Vec4
        }
    }
    
    pub fn to_val(self) -> Val {
        match self {
            Lit::Bool(v) => Val::Bool(v),
            Lit::Int(v) => Val::Int(v),
            Lit::Float(v) => Val::Float(v),
            Lit::Color(v) => Val::Vec4(Vec4::from_u32(v))
        }
    }
    
    pub fn from_token(token: Token) -> Option<Lit> {
        match token {
            Token::Bool(v) => Some(Lit::Bool(v)),
            Token::Int(v) => Some(Lit::Int(v as i32)),
            Token::Float(v) => Some(Lit::Float(v as f32)),
            Token::Color(v) => Some(Lit::Color(v)),
            _ => None
        }
    }
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Lit::Bool(lit) => write!(f, "{}", lit),
            Lit::Int(lit) => write!(f, "{}", lit),
            Lit::Float(lit) => write!(f, "{}", PrettyPrintedF32(*lit)),
            Lit::Color(lit) => {
                let v = Vec4::from_u32(*lit);
                write!(
                    f,
                    "vec4({},{},{},{})",
                    PrettyPrintedF32(v.x),
                    PrettyPrintedF32(v.y),
                    PrettyPrintedF32(v.z),
                    PrettyPrintedF32(v.w)
                )
            }
        }
    }
}


impl Ident {
    pub fn to_id(self) -> Id {self.0}
    pub fn to_ident_path(self) -> IdentPath {
        IdentPath::from_ident(self)
    }
}

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Default, Copy, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct IdentPath {
    pub segs: [Id; 6],
    pub len: usize
}

#[derive(Clone, Copy, Default, Eq, PartialEq, Debug)]
pub struct IdentPathWithSpan {
    pub span: Span,
    pub ident_path: IdentPath,
}


impl IdentPath {
    
    pub fn from_ident(ident: Ident) -> Self {
        let mut p = IdentPath::default();
        p.segs[0] = ident.0;
        p.len = 1;
        p
    }
    
    pub fn from_two(ident1: Ident, ident2: Ident) -> Self {
        let mut p = IdentPath::default();
        p.segs[0] = ident1.0;
        p.segs[1] = ident2.0;
        p.len = 2;
        p
    }
    
    pub fn from_three(ident1: Ident, ident2: Ident, ident3: Ident) -> Self {
        let mut p = IdentPath::default();
        p.segs[0] = ident1.0;
        p.segs[1] = ident2.0;
        p.segs[1] = ident3.0;
        p.len = 3;
        p
    }
    
    pub fn from_array(idents: &[Ident]) -> Self {
        let mut p = IdentPath::default();
        for i in 0..idents.len() {
            p.segs[i] = idents[i].0;
        }
        p.len = idents.len();
        p
    }
    
    pub fn to_struct_fn_ident(&self) -> Ident {
        let mut s = String::new();
        for i in 0..self.len {
            if i != 0 {
                let _ = write!(s, "_").unwrap();
            }
            let _ = write!(s, "{}", self.segs[i]);
        }
        Ident(Id::from_str(&s).panic_collision(&s))
    }
    
    pub fn from_str(value: &str) -> Self {
        let mut p = IdentPath::default();
        p.segs[0] = Id::from_str(value);
        p.len = 1;
        p
    }
    
    pub fn is_self_scope(&self) -> bool {
        self.len > 1 && self.segs[0] == id!(self)
    }
    
    pub fn len(&self) -> usize {
        self.len
    }
    
    pub fn push(&mut self, ident: Ident) -> bool {
        if self.len >= self.segs.len() {
            return false
        }
        self.segs[self.len] = ident.0;
        self.len += 1;
        return true
    }
    
    
    pub fn get_single(&self) -> Option<Ident> {
        if self.len != 1 {
            return None
        }
        return Some(Ident(self.segs[0]))
    }
}

impl fmt::Debug for IdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in 0..self.len {
            if i != 0 {
                let _ = write!(f, "::").unwrap();
            }
            let _ = write!(f, "{}", self.segs[i]);
        }
        Ok(())
    }
}

impl fmt::Display for IdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in 0..self.len {
            if i != 0 {
                let _ = write!(f, "::").unwrap();
            }
            let _ = write!(f, "{}", self.segs[i]);
        }
        Ok(())
    }
}

impl Val {
    pub fn to_bool(&self) -> Option<bool> {
        match *self {
            Val::Bool(val) => Some(val),
            _ => None,
        }
    }
    
    pub fn to_int(&self) -> Option<i32> {
        match *self {
            Val::Int(val) => Some(val),
            _ => None,
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Val::Bool(val) => write!(f, "{}", val),
            Val::Int(val) => write!(f, "{}", val),
            Val::Float(v) => write!(f, "{}", PrettyPrintedF32(v)),
            Val::Vec4(val) => write!(f, "{}", val),
        }
    }
}

