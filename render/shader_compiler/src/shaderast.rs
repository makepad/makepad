use crate::env::VarKind;
use crate::ident::{Ident, IdentPath, QualifiedIdentPath, IdentPathWithSpan};
use crate::lit::{Lit};
use makepad_live_parser::Span;
use makepad_live_parser::*;
use crate::ty::{Ty,TyLit,TyExpr};
use crate::val::Val;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Debug, Default)]
pub struct ShaderAst {
    //pub qualified_ident_path: QualifiedIdentPath,
    pub debug: bool,
    pub default_geometry: Option<IdentPathWithSpan>,
    pub draw_input: Option<(Span, QualifiedIdentPath)>,
    pub decls: Vec<Decl>,
    pub uses: Vec<IdentPathWithSpan>,
    // generated
    pub const_table: RefCell<Option<Vec<f32 >> >,
    pub const_table_spans: RefCell<Option<Vec<(usize, Span) >> >,
    pub livestyle_uniform_deps: RefCell<Option<BTreeSet<(Ty, QualifiedIdentPath) >> >,
}

impl ShaderAst {
}

#[derive(Clone, Debug)]
pub enum Decl {
    Geometry(GeometryDecl),
    Const(ConstDecl),
    Fn(FnDecl),
    Instance(InstanceDecl),
    Struct(StructDecl),
    Texture(TextureDecl),
    Uniform(UniformDecl),
    Varying(VaryingDecl),
}

#[derive(Clone, Debug)]
pub struct GeometryDecl {
    pub is_used_in_fragment_shader: Cell<Option<bool >>,
    pub span: Span,
    pub ident: Ident,
    pub qualified_ident_path: QualifiedIdentPath,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub expr: Expr,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub span: Span,
    pub return_ty: RefCell<Option<Ty >>,
    pub is_used_in_vertex_shader: Cell<Option<bool >>,
    pub is_used_in_fragment_shader: Cell<Option<bool >>,
    pub callees: RefCell<Option<BTreeSet<IdentPath >> >,
    pub uniform_block_deps: RefCell<Option<BTreeSet<Ident >> >,
    pub has_texture_deps: Cell<Option<bool >>,
    pub geometry_deps: RefCell<Option<BTreeSet<Ident >> >,
    pub instance_deps: RefCell<Option<BTreeSet<Ident >> >,
    pub has_varying_deps: Cell<Option<bool >>,
    pub builtin_deps: RefCell<Option<BTreeSet<Ident >> >,
    pub cons_fn_deps: RefCell<Option<BTreeSet<(TyLit, Vec<Ty>) >> >,
    pub ident_path: IdentPath,
    pub params: Vec<Param>,
    pub return_ty_expr: Option<TyExpr>,
    pub block: Block,
}

#[derive(Clone, Debug)]
pub struct InstanceDecl {
    pub is_used_in_fragment_shader: Cell<Option<bool >>,
    pub span: Span,
    pub ident: Ident,
    pub qualified_ident_path: QualifiedIdentPath,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub span: Span,
    pub ident: Ident,
    pub fields: Vec<Field>,
}

impl StructDecl {
    pub fn find_field(&self, ident: Ident) -> Option<&Field> {
        self.fields.iter().find( | field | field.ident == ident)
    }
}

#[derive(Clone, Debug)]
pub struct TextureDecl {
    pub span: Span,
    pub ident: Ident,
    pub qualified_ident_path: QualifiedIdentPath,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct UniformDecl {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub qualified_ident_path: QualifiedIdentPath,
    pub block_ident: Option<Ident>,
}

#[derive(Clone, Debug)]
pub struct VaryingDecl {
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
pub struct Field {
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
    MethodCall {
        span: Span,
        ident: Ident,
        arg_exprs: Vec<Expr>,
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
    Call {
        span: Span,
        ident_path: IdentPath,
        arg_exprs: Vec<Expr>,
    },
    MacroCall {
        span: Span,
        analysis: Cell<Option<MacroCallAnalysis >>,
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

impl BinOp {
    pub fn from_assign_op(token:Token) -> Option<BinOp> {
        match token {
            token_punct!(=) => Some(BinOp::Assign),
            token_punct!(+=) =>Some(BinOp::AddAssign),
            token_punct!(-=) => Some(BinOp::SubAssign),
            token_punct!(*=) => Some(BinOp::MulAssign),
            token_punct!(/=) => Some(BinOp::DivAssign),
            _ => None,
        }
    }
    
    pub fn from_or_op(token:Token) -> Option<BinOp> {
        match token {
            token_punct!(||) => Some(BinOp::Or),
            _ => None,
        }
    }
    
    pub fn from_and_op(token:Token) -> Option<BinOp> {
        match token {
            token_punct!(&&) => Some(BinOp::And),
            _ => None,
        }
    }
    
    pub fn from_eq_op(token:Token) -> Option<BinOp> {
        match token {
            token_punct!(==) => Some(BinOp::Eq),
            token_punct!(!=) => Some(BinOp::Ne),
            _ => None,
        }
    }
    
    pub fn from_rel_op(token:Token) -> Option<BinOp> {
        match token {
            token_punct!(<) => Some(BinOp::Lt),
            token_punct!(<=) => Some(BinOp::Le),
            token_punct!(>) => Some(BinOp::Gt),
            token_punct!(>=) => Some(BinOp::Ge),
            _ => None,
        }
    }
    
    pub fn from_add_op(token:Token) -> Option<BinOp> {
        match token {
            token_punct!(+) => Some(BinOp::Add),
            token_punct!(-) => Some(BinOp::Sub),
            _ => None,
        }
    }
    
    pub fn from_mul_op(token:Token) -> Option<BinOp> {
        match token {
            token_punct!(*) => Some(BinOp::Mul),
            token_punct!(/) => Some(BinOp::Div),
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

#[derive(Clone, Copy, Debug)]
pub enum UnOp {
    Not,
    Neg,
}

impl UnOp{
    pub fn from_un_op(token:Token) -> Option<UnOp> {
        match token {
            token_punct!(!) =>Some(UnOp::Not),
            token_punct!(-)=> Some(UnOp::Neg),
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
