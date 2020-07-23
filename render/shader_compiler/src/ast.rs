use crate::ident::Ident;
use crate::lit::{Lit, TyLit};
use crate::span::Span;
use crate::ty::Ty;
use crate::val::Val;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fmt;

#[derive(Clone, Debug)]
pub struct ShaderAst {
    pub decls: Vec<Decl>,
}

impl ShaderAst {
    pub fn new() -> ShaderAst {
        ShaderAst {
            decls: Vec::new()
        }
    }

    pub fn find_attribute_decl(&self, ident: Ident) -> Option<&AttributeDecl> {
        self.decls.iter().find_map(|decl| {
            match decl {
                Decl::Attribute(decl) => Some(decl),
                _ => None,
            }
            .filter(|decl| decl.ident == ident)
        })
    }

    pub fn find_const_decl(&self, ident: Ident) -> Option<&ConstDecl> {
        self.decls.iter().find_map(|decl| {
            match decl {
                Decl::Const(decl) => Some(decl),
                _ => None,
            }
            .filter(|decl| decl.ident == ident)
        })
    }

    pub fn find_fn_decl(&self, ident: Ident) -> Option<&FnDecl> {
        self.decls.iter().rev().find_map(|decl| {
            match decl {
                Decl::Fn(decl) => Some(decl),
                _ => None,
            }
            .filter(|decl| decl.ident == ident)
        })
    }

    pub fn find_instance_decl(&self, ident: Ident) -> Option<&InstanceDecl> {
        self.decls.iter().find_map(|decl| {
            match decl {
                Decl::Instance(decl) => Some(decl),
                _ => None,
            }
            .filter(|decl| decl.ident == ident)
        })
    }

    pub fn find_struct_decl(&self, ident: Ident) -> Option<&StructDecl> {
        self.decls.iter().find_map(|decl| {
            match decl {
                Decl::Struct(decl) => Some(decl),
                _ => None,
            }
            .filter(|decl| decl.ident == ident)
        })
    }

    pub fn find_uniform_decl(&self, ident: Ident) -> Option<&UniformDecl> {
        self.decls.iter().find_map(|decl| {
            match decl {
                Decl::Uniform(decl) => Some(decl),
                _ => None,
            }
            .filter(|decl| decl.ident == ident)
        })
    }

    pub fn find_varying_decl(&self, ident: Ident) -> Option<&VaryingDecl> {
        self.decls.iter().find_map(|decl| {
            match decl {
                Decl::Varying(decl) => Some(decl),
                _ => None,
            }
            .filter(|decl| decl.ident == ident)
        })
    }
}

#[derive(Clone, Debug)]
pub enum Decl {
    Attribute(AttributeDecl),
    Const(ConstDecl),
    Fn(FnDecl),
    Instance(InstanceDecl),
    Struct(StructDecl),
    Texture(TextureDecl),
    Uniform(UniformDecl),
    Varying(VaryingDecl),
}

#[derive(Clone, Debug)]
pub struct AttributeDecl {
    pub is_used_in_fragment_shader: Cell<Option<bool>>,
    pub span: Span,
    pub ident: Ident,
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
    pub return_ty: RefCell<Option<Ty>>,
    pub is_used_in_vertex_shader: Cell<Option<bool>>,
    pub is_used_in_fragment_shader: Cell<Option<bool>>,
    pub callees: RefCell<Option<HashSet<Ident>>>,
    pub uniform_block_deps: RefCell<Option<HashSet<Ident>>>,
    pub has_texture_deps: Cell<Option<bool>>,
    pub attribute_deps: RefCell<Option<HashSet<Ident>>>,
    pub instance_deps: RefCell<Option<HashSet<Ident>>>,
    pub has_varying_deps: Cell<Option<bool>>,
    pub builtin_deps: RefCell<Option<HashSet<Ident>>>,
    pub cons_fn_deps: RefCell<Option<HashSet<(TyLit, Vec<Ty>)>>>,
    pub ident: Ident,
    pub params: Vec<Param>,
    pub return_ty_expr: Option<TyExpr>,
    pub block: Block,
}

#[derive(Clone, Debug)]
pub struct InstanceDecl {
    pub is_used_in_fragment_shader: Cell<Option<bool>>,
    pub span: Span,
    pub ident: Ident,
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
        self.fields.iter().find(|field| field.ident == ident)
    }
}

#[derive(Clone, Debug)]
pub struct TextureDecl {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct UniformDecl {
    pub span: Span,
    pub ident: Ident,
    pub ty_expr: TyExpr,
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
        block_if_false: Option<Box<Block>>,
    },
    Let {
        span: Span,
        ty: RefCell<Option<Ty>>,
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
pub struct TyExpr {
    pub ty: RefCell<Option<Ty>>,
    pub kind: TyExprKind,
}

#[derive(Clone, Debug)]
pub enum TyExprKind {
    Array {
        span: Span,
        elem_ty_expr: Box<TyExpr>,
        len: u32
    },
    Var {
        span: Span,
        ident: Ident
    },
    Lit {
        span: Span,
        ty_lit: TyLit
    },
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub ty: RefCell<Option<Ty>>,
    pub val: RefCell<Option<Val>>,
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
        ident: Ident,
        arg_exprs: Vec<Expr>,
    },
    MacroCall {
        analysis: Cell<Option<MacroCallAnalysis>>,
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
        ident: Ident,
    },
    Lit {
        span: Span,
        lit: Lit,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum MacroCallAnalysis{
    Color{r:f32,g:f32,b:f32,a:f32}
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
