use crate::ident::Ident;
use crate::lit::Lit;
use crate::ty_lit::TyLit;
use std::fmt;

#[derive(Clone, Debug)]
pub struct ParsedShader {
    pub decls: Vec<Decl>,
}

#[derive(Clone, Debug)]
pub enum Decl {
    Attribute(AttributeDecl),
    Fn(FnDecl),
    Struct(StructDecl),
    Uniform(UniformDecl),
    Varying(VaryingDecl),
}

#[derive(Clone, Debug)]
pub struct AttributeDecl {
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub ident: Ident,
    pub params: Vec<Param>,
    pub return_ty_expr: Option<TyExpr>,
    pub block: Block,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub ident: Ident,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug)]
pub struct UniformDecl {
    pub ident: Ident,
    pub ty_expr: TyExpr,
    pub block_ident: Option<Ident>,
}

#[derive(Clone, Debug)]
pub struct VaryingDecl {
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub ident: Ident,
    pub ty_expr: TyExpr,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Break(BreakStmt),
    Continue(ContinueStmt),
    For(ForStmt),
    If(IfStmt),
    Let(LetStmt),
    Return(ReturnStmt),
    Block(BlockStmt),
    Expr(ExprStmt),
}

#[derive(Clone, Copy, Debug)]
pub struct BreakStmt;

#[derive(Clone, Copy, Debug)]
pub struct ContinueStmt;

#[derive(Clone, Debug)]
pub struct ForStmt {
    pub ident: Ident,
    pub from_expr: Expr,
    pub to_expr: Expr,
    pub step_expr: Option<Expr>,
    pub block: Block,
}

#[derive(Clone, Debug)]
pub struct IfStmt {
    pub expr: Expr,
    pub block_if_true: Block,
    pub block_if_false: Option<Block>,
}

#[derive(Clone, Debug)]
pub struct LetStmt {
    pub ident: Ident,
    pub ty_expr: Option<TyExpr>,
    pub expr: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct ReturnStmt {
    pub expr: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct BlockStmt {
    pub block: Block,
}

#[derive(Clone, Debug)]
pub struct ExprStmt {
    pub expr: Expr,
}

#[derive(Clone, Debug)]
pub enum TyExpr {
    Array(ArrayTyExpr),
    Struct(StructTyExpr),
    TyLit(TyLit),
}

#[derive(Clone, Debug)]
pub struct ArrayTyExpr {
    pub elem_ty_expr: Box<TyExpr>,
    pub len: u32,
}

#[derive(Clone, Debug)]
pub struct StructTyExpr {
    pub ident: Ident,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Cond(CondExpr),
    Bin(BinExpr),
    Un(UnExpr),
    Index(IndexExpr),
    Member(MemberExpr),
    Call(CallExpr),
    ConsCall(ConsCallExpr),
    Var(VarExpr),
    Lit(Lit),
}

#[derive(Clone, Debug)]
pub struct CondExpr {
    pub x: Box<Expr>,
    pub y: Box<Expr>,
    pub z: Box<Expr>,
}

#[derive(Clone, Debug)]
pub struct BinExpr {
    pub x: Box<Expr>,
    pub op: BinOp,
    pub y: Box<Expr>,
}

#[derive(Clone, Debug)]
pub struct UnExpr {
    pub op: UnOp,
    pub x: Box<Expr>,
}

#[derive(Clone, Debug)]
pub struct IndexExpr {
    pub x: Box<Expr>,
    pub i: Box<Expr>,
}

#[derive(Clone, Debug)]
pub struct MemberExpr {
    pub x: Box<Expr>,
    pub ident: Ident,
}

#[derive(Clone, Debug)]
pub struct CallExpr {
    pub ident: Ident,
    pub xs: Vec<Expr>,
}

#[derive(Clone, Debug)]
pub struct ConsCallExpr {
    pub ty_lit: TyLit,
    pub xs: Vec<Expr>,
}

#[derive(Clone, Copy, Debug)]
pub struct VarExpr {
    pub ident: Ident,
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
