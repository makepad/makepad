//! The formula language's syntax tree: the scripting dialect patches store
//! (a small dynamic language: numbers, booleans, strings, nil, tables,
//! first-class functions). The parser accepts the whole grammar; the
//! compiler ([`super::compile`]) decides what it can translate and reports
//! the rest as load errors with a line number.

use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    /// Floored remainder: `a - floor(a / b) * b`.
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Len,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Nil,
    True,
    False,
    Num(f64),
    Str(Rc<str>),
    Vararg,
    Func(Rc<FuncBody>),
    Table(Vec<Field>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Un(UnOp, Box<Expr>),
    Name(Rc<str>),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    /// `obj:name(args)`.
    Method(Box<Expr>, Rc<str>, Vec<Expr>),
    /// `(e)`: truncates a multi-value expression to one value.
    Paren(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum Field {
    /// `[k] = v` and `name = v` (the name as a string key).
    Keyed(Expr, Expr),
    /// A positional value (keys 1, 2, ...).
    Pos(Expr),
}

#[derive(Clone, Debug)]
pub struct FuncBody {
    pub params: Vec<Rc<str>>,
    pub vararg: bool,
    pub body: Block,
    pub line: u32,
    /// `function a.b:c()`: a method (an implicit first parameter `self`).
    pub name: Option<Rc<str>>,
}

pub type Block = Vec<Stat>;

#[derive(Clone, Debug)]
pub struct Stat {
    pub kind: StatKind,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum StatKind {
    Local(Vec<Rc<str>>, Vec<Expr>),
    /// `local function f`: `f` is in scope inside its own body.
    LocalFunc(Rc<str>, Rc<FuncBody>),
    Assign(Vec<Expr>, Vec<Expr>),
    /// A call statement (results dropped).
    Call(Expr),
    Do(Block),
    While(Expr, Block),
    Repeat(Block, Expr),
    If(Vec<(Expr, Block)>, Option<Block>),
    NumFor(Rc<str>, Expr, Expr, Option<Expr>, Block),
    GenFor(Vec<Rc<str>>, Vec<Expr>, Block),
    Return(Vec<Expr>),
    Break,
}
