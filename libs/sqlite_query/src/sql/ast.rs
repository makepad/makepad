//! SQL syntax tree for the statement shapes this engine runs.
//!
//! The tree stays close to SQLite's grammar (<https://www.sqlite.org/lang.html>)
//! so the parser can be a straight transcription and the planner can pattern
//! match on the same shapes SQLite's own planner sees.

use crate::sql::lexer::ParamRef;
use crate::value::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Select(Box<SelectStmt>),
    Insert(Box<InsertStmt>),
    Update(Box<UpdateStmt>),
    Delete(Box<DeleteStmt>),
    /// `CREATE TABLE`: the statement text is kept verbatim because that is what
    /// `sqlite_master` stores and what every reader re-parses.
    CreateTable {
        name: String,
        if_not_exists: bool,
        sql: String,
    },
    CreateIndex {
        name: String,
        table: String,
        unique: bool,
        if_not_exists: bool,
        sql: String,
    },
    DropTable {
        name: String,
        if_exists: bool,
    },
    DropIndex {
        name: String,
        if_exists: bool,
    },
    AlterAddColumn {
        table: String,
        /// The column definition as written, appended to the stored DDL.
        column_sql: String,
    },
    AlterRenameTable {
        table: String,
        new_name: String,
    },
    Begin(TxKind),
    Commit,
    Rollback,
    Pragma {
        name: String,
        /// `PRAGMA x = value` or `PRAGMA x(value)`.
        value: Option<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxKind {
    Deferred,
    Immediate,
    Exclusive,
}

/// What to do when a row conflicts with a uniqueness constraint.
#[derive(Debug, Clone, PartialEq)]
pub enum OnConflict {
    /// The default: fail the statement.
    Abort,
    Ignore,
    Replace,
    /// `ON CONFLICT [(cols)] DO NOTHING`
    DoNothing { target: Vec<String> },
    /// `ON CONFLICT [(cols)] DO UPDATE SET ... [WHERE ...]`
    DoUpdate {
        target: Vec<String>,
        sets: Vec<(String, Expr)>,
        where_clause: Option<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub table: String,
    /// Named columns; empty means every column in declaration order.
    pub columns: Vec<String>,
    pub source: InsertSource,
    pub on_conflict: OnConflict,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertSource {
    Values(Vec<Vec<Expr>>),
    Select(Box<SelectStmt>),
    DefaultValues,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStmt {
    pub table: String,
    pub sets: Vec<(String, Expr)>,
    pub where_clause: Option<Expr>,
    /// `UPDATE OR IGNORE` / `OR REPLACE`.
    pub or_conflict: OnConflict,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteStmt {
    pub table: String,
    pub where_clause: Option<Expr>,
}

// ---------------------------------------------------------------------------
// SELECT
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct SelectStmt {
    pub distinct: bool,
    pub columns: Vec<ResultColumn>,
    pub from: Option<FromClause>,
    pub where_clause: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
    pub order_by: Vec<OrderTerm>,
    pub limit: Option<Expr>,
    pub offset: Option<Expr>,
    /// `UNION [ALL] / INTERSECT / EXCEPT` continuation, if any.
    pub compound: Option<Box<Compound>>,
}

impl SelectStmt {
    pub fn empty() -> SelectStmt {
        SelectStmt {
            distinct: false,
            columns: Vec::new(),
            from: None,
            where_clause: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            compound: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compound {
    pub op: CompoundOp,
    pub select: SelectStmt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp {
    Union,
    UnionAll,
    Intersect,
    Except,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResultColumn {
    /// `*`
    Star,
    /// `t.*`
    TableStar(String),
    Expr { expr: Expr, alias: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FromClause {
    pub base: TableRef,
    pub joins: Vec<Join>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableRef {
    Named {
        name: String,
        alias: Option<String>,
    },
    Subquery {
        select: Box<SelectStmt>,
        alias: Option<String>,
    },
}

impl TableRef {
    /// The name rows of this item are addressed by in expressions.
    pub fn binding(&self) -> Option<&str> {
        match self {
            TableRef::Named { name, alias } => Some(alias.as_deref().unwrap_or(name)),
            TableRef::Subquery { alias, .. } => alias.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    pub kind: JoinKind,
    pub table: TableRef,
    pub constraint: JoinConstraint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
    Cross,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinConstraint {
    On(Expr),
    Using(Vec<String>),
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderTerm {
    pub expr: Expr,
    pub desc: bool,
    /// Explicit `NULLS FIRST` / `NULLS LAST`; SQLite's default is nulls first
    /// for ASC and nulls last for DESC.
    pub nulls_first: Option<bool>,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Value),
    Param(ParamRef),
    Column {
        table: Option<String>,
        name: String,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `expr IS [NOT] NULL`, also spelled ISNULL / NOTNULL.
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    InSelect {
        expr: Box<Expr>,
        select: Box<SelectStmt>,
        negated: bool,
    },
    Exists {
        select: Box<SelectStmt>,
        negated: bool,
    },
    /// Scalar subquery `(SELECT ...)`.
    Subquery(Box<SelectStmt>),
    Function {
        name: String,
        args: Vec<Expr>,
        distinct: bool,
        /// `count(*)`
        star: bool,
    },
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        else_result: Option<Box<Expr>>,
    },
    Cast {
        expr: Box<Expr>,
        type_name: String,
    },
    Collate {
        expr: Box<Expr>,
        collation: String,
    },
    /// LIKE / GLOB with an optional ESCAPE.
    Like {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        escape: Option<Box<Expr>>,
        negated: bool,
        glob: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Identity,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Is,
    IsNot,
    BitAnd,
    BitOr,
    Shl,
    Shr,
}

impl BinOp {
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}
