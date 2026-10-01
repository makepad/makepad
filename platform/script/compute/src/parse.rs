//! The audio shader front end, part 1: lexing and parsing Splash syntax
//! into an AST with byte spans (spans are relative to the shader's code
//! string, so an editor can underline an error in place).

use crate::ShaderError;

#[derive(Clone, Debug, PartialEq)]
pub enum Tk {
    Ident(String),
    /// A number and whether it was written without a fraction/exponent.
    Num(f64, bool),
    Punct(&'static str),
    /// A string (one line, no escapes): only `use lib("id", "rev")`.
    Str(String),
    /// A colour literal (`#eee9df`, `#fff`, `#ff000080`) as Splash reads
    /// it: `0xRRGGBBAA`, the channels sRGB-encoded as written.
    Color(u32),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tk: Tk,
    pub start: usize,
    pub end: usize,
    /// A newline separates this token from the previous one.
    pub nl: bool,
}

const PUNCTS: &[&str] = &[
    ">>>", "..", "=>", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "<<", ">>", "->", "+",
    "-", "*", "/", "%", "=", "<", ">", "!", "&", "|", "^", "(", ")", "{", "}", "[", "]", ",", ";", ":",
    ".",
];

pub fn lex(src: &str) -> Result<Vec<Token>, ShaderError> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut nl = true;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            nl = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                if b[i] == b'\n' {
                    nl = true;
                }
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        let start = i;
        let tk = if c == b'0' && matches!(b.get(i + 1), Some(b'x') | Some(b'X')) {
            i += 2;
            while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
                i += 1;
            }
            let text: String = src[start + 2..i].chars().filter(|c| *c != '_').collect();
            match u32::from_str_radix(&text, 16) {
                Ok(v) => Tk::Num(v as i32 as f64, true),
                Err(_) => return Err(ShaderError::new(start, i, format!("bad hex number `{}`", &src[start..i]))),
            }
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let mut int = true;
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() || d == b'_' {
                    i += 1;
                } else if d == b'.' && b.get(i + 1) != Some(&b'.') && !int_is_method(b, i) {
                    int = false;
                    i += 1;
                } else if (d == b'e' || d == b'E')
                    && (b.get(i + 1).is_some_and(u8::is_ascii_digit)
                        || (matches!(b.get(i + 1), Some(b'-') | Some(b'+'))
                            && b.get(i + 2).is_some_and(u8::is_ascii_digit)))
                {
                    int = false;
                    i += 2;
                } else {
                    break;
                }
            }
            let text: String = src[start..i].chars().filter(|c| *c != '_').collect();
            match text.parse::<f64>() {
                Ok(v) => Tk::Num(v, int),
                Err(_) => return Err(ShaderError::new(start, i, format!("bad number `{}`", &src[start..i]))),
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            Tk::Ident(src[start..i].to_string())
        } else if c == b'#' {
            // A colour, as the document's VM reads it: `#` (an optional `x`)
            // and 1, 2, 3, 4, 6 or 8 hex digits.
            i += 1;
            if b.get(i) == Some(&b'x') {
                i += 1;
            }
            let digits = i;
            while i < b.len() && b[i].is_ascii_hexdigit() && i - digits < 8 {
                i += 1;
            }
            if i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                return Err(ShaderError::new(start, i + 1, format!("bad colour `{}`: a colour is `#` and 1, 2, 3, 4, 6 or 8 hex digits", &src[start..=i])));
            }
            match color_hex(&src[digits..i]) {
                Some(v) => Tk::Color(v),
                None => return Err(ShaderError::new(start, i.max(start + 1), format!("bad colour `{}`: a colour is `#` and 1, 2, 3, 4, 6 or 8 hex digits", &src[start..i]))),
            }
        } else if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' && b[i] != b'\n' {
                i += 1;
            }
            if b.get(i) != Some(&b'"') || i - start > 256 {
                return Err(ShaderError::new(start, i.max(start + 1), "unclosed or overlong string".into()));
            }
            i += 1;
            Tk::Str(src[start + 1..i - 1].to_string())
        } else {
            let rest = &src[i..];
            let Some(p) = PUNCTS.iter().find(|p| rest.starts_with(**p)) else {
                let ch = rest.chars().next().unwrap();
                let msg = if ch == '\'' {
                    "no strings here (a string only names a library: `use lib(\"id\", \"rev\")`)".to_string()
                } else {
                    format!("unexpected character `{}`", ch)
                };
                return Err(ShaderError::new(start, start + ch.len_utf8(), msg));
            };
            i += p.len();
            Tk::Punct(p)
        };
        out.push(Token { tk, start, end: i, nl });
        nl = false;
    }
    out.push(Token { tk: Tk::Eof, start: b.len(), end: b.len(), nl: true });
    Ok(out)
}

/// A colour's hex digits as `0xRRGGBBAA`, as Splash reads them (`#w`,
/// `#ww`, `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`).
fn color_hex(d: &str) -> Option<u32> {
    let h = |k: usize| u32::from_str_radix(&d[k..k + 1], 16).ok();
    let hh = |k: usize| u32::from_str_radix(&d[k..k + 2], 16).ok();
    Some(match d.len() {
        1 => {
            let v = h(0)? * 17;
            (v << 24) | (v << 16) | (v << 8) | 0xff
        }
        2 => {
            let v = hh(0)?;
            (v << 24) | (v << 16) | (v << 8) | 0xff
        }
        3 => ((h(0)? * 17) << 24) | ((h(1)? * 17) << 16) | ((h(2)? * 17) << 8) | 0xff,
        4 => ((h(0)? * 17) << 24) | ((h(1)? * 17) << 16) | ((h(2)? * 17) << 8) | (h(3)? * 17),
        6 => (hh(0)? << 24) | (hh(2)? << 16) | (hh(4)? << 8) | 0xff,
        8 => (hh(0)? << 24) | (hh(2)? << 16) | (hh(4)? << 8) | hh(6)?,
        _ => return None,
    })
}

/// `1.max(2)`-style method calls on integers are not a thing here, but
/// `x.0` is not either; a dot followed by a letter ends the number.
fn int_is_method(b: &[u8], i: usize) -> bool {
    b.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
}

// =========================================================================
// AST
// =========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeAnn {
    F32,
    F64,
    I32,
    Bool,
    Vec2,
    Vec3,
    Vec4,
    Mat4,
}

#[derive(Clone, Debug)]
pub enum Item {
    Let { name: String, ann: Option<TypeAnn>, value: Expr, span: Span },
    Var { name: String, ann: Option<TypeAnn>, value: Expr, span: Span },
    Struct { name: String, fields: Vec<(String, Expr)>, span: Span },
    Fn(FnDecl),
    /// `use path.*` (glob), `use path as alias`, `use path` (alias: the
    /// last part), `use lib("id", "rev") as alias` (path `lib:id@rev`).
    Use { path: String, glob: bool, alias: Option<String>, span: Span },
}

impl Item {
    pub fn name(&self) -> &str {
        match self {
            Item::Let { name, .. } | Item::Var { name, .. } | Item::Struct { name, .. } => name,
            Item::Fn(f) => &f.name,
            Item::Use { path, .. } => path,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<(String, Option<TypeAnn>)>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let { name: String, ann: Option<TypeAnn>, value: Expr, span: Span },
    Assign { target: Expr, op: AssignOp, value: Expr, span: Span },
    Expr(Expr),
    For { var: String, from: Expr, to: Expr, body: Vec<Stmt>, span: Span },
    While { cond: Expr, body: Vec<Stmt>, span: Span },
    Loop { body: Vec<Stmt>, span: Span },
    Break(Span),
    Continue(Span),
    Return(Option<Expr>, Span),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    /// `>>>`: logical (unsigned) shift right.
    ShrU,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Num(f64, bool),
    Bool(bool),
    Ident(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// `if c {..} elif c {..} else {..}`: (cond, body) pairs and the else.
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    /// Arms: (patterns, body); a `None` pattern list is `_`.
    Match(Box<Expr>, Vec<(Option<Vec<Expr>>, Vec<Stmt>)>),
    Block(Vec<Stmt>),
    ArrayRepeat(Box<Expr>, Box<Expr>),
    ArrayList(Vec<Expr>),
    StructLit(String, Vec<(String, Expr)>),
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

// =========================================================================
// Parser
// =========================================================================

/// `a` or `a.b.c` (plain names only): the module path of a qualified call.
fn dotted(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field(b, n) => dotted(b).map(|q| format!("{}.{}", q, n)),
        _ => None,
    }
}

pub struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
}

type PResult<T> = Result<T, ShaderError>;

impl<'a> Parser<'a> {
    pub fn new(toks: &'a [Token]) -> Self {
        Parser { toks, pos: 0 }
    }

    fn peek(&self) -> &Tk {
        &self.toks[self.pos].tk
    }

    fn tok(&self) -> &Token {
        &self.toks[self.pos]
    }

    fn prev_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.toks[self.pos - 1].end
        }
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Tk::Punct(q) if *q == p)
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tk::Ident(s) if s == kw)
    }

    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        let t = self.tok();
        Err(ShaderError::new(t.start, t.end.max(t.start + 1), msg.into()))
    }

    fn expect(&mut self, p: &str) -> PResult<()> {
        if self.eat(p) {
            Ok(())
        } else {
            let found = describe(self.peek());
            self.err(format!("expected `{}`, found {}", p, found))
        }
    }

    fn ident(&mut self) -> PResult<String> {
        match self.peek().clone() {
            Tk::Ident(s) => {
                self.bump();
                Ok(s)
            }
            other => self.err(format!("expected a name, found {}", describe(&other))),
        }
    }

    fn skip_seps(&mut self) {
        while self.is(";") || self.is(",") {
            self.bump();
        }
    }

    pub fn items(&mut self) -> PResult<Vec<Item>> {
        let mut items = Vec::new();
        loop {
            self.skip_seps();
            if matches!(self.peek(), Tk::Eof) {
                return Ok(items);
            }
            let start = self.tok().start;
            if self.is_kw("let") || self.is_kw("var") || self.is_kw("const") {
                let is_var = self.is_kw("var");
                self.bump();
                let name = self.ident()?;
                let ann = self.type_ann()?;
                self.expect("=")?;
                let value = self.expr()?;
                let span = Span { start, end: self.prev_end() };
                items.push(if is_var {
                    Item::Var { name, ann, value, span }
                } else {
                    Item::Let { name, ann, value, span }
                });
            } else if self.is_kw("struct") {
                self.bump();
                let name = self.ident()?;
                self.expect("{")?;
                let fields = self.fields()?;
                items.push(Item::Struct { name, fields, span: Span { start, end: self.prev_end() } });
            } else if self.is_kw("fn") {
                self.bump();
                items.push(Item::Fn(self.fn_decl(start)?));
            } else if self.is_kw("use") {
                self.bump();
                items.push(self.use_item(start)?);
            } else {
                return self.err("expected `let`, `const`, `var`, `struct`, `fn` or `use` at the top level");
            }
        }
    }

    /// After `use`: a dotted path (optionally ending in `.*`) or
    /// `lib("id", "rev")`, then an optional `as alias`.
    fn use_item(&mut self, start: usize) -> PResult<Item> {
        let mut glob = false;
        let path = if self.is_kw("lib") {
            self.bump();
            self.expect("(")?;
            let mut parts = Vec::new();
            loop {
                self.skip_seps();
                if self.eat(")") {
                    break;
                }
                match self.peek().clone() {
                    Tk::Str(s) => {
                        self.bump();
                        parts.push(s);
                    }
                    other => return self.err(format!("lib(\"id\", \"rev\") takes strings, found {}", describe(&other))),
                }
            }
            if parts.is_empty() || parts.len() > 2 {
                return self.err("lib(\"id\", \"rev\")");
            }
            if self.eat(".") {
                self.expect("*")?;
                glob = true;
            }
            format!("lib:{}", parts.join("@"))
        } else {
            let mut parts = vec![self.ident()?];
            while self.eat(".") {
                if self.eat("*") {
                    glob = true;
                    break;
                }
                parts.push(self.ident()?);
            }
            parts.join(".")
        };
        let alias = if self.is_kw("as") {
            self.bump();
            Some(self.ident()?)
        } else {
            None
        };
        if glob && alias.is_some() {
            return self.err("`use m.*` brings the names in unqualified; `use m as a` names the module: not both");
        }
        Ok(Item::Use { path, glob, alias, span: Span { start, end: self.prev_end() } })
    }

    fn type_ann(&mut self) -> PResult<Option<TypeAnn>> {
        if !self.eat(":") {
            return Ok(None);
        }
        let t = self.tok().clone();
        let name = self.ident()?;
        Ok(Some(match name.as_str() {
            "f32" | "float" => TypeAnn::F32,
            "f64" => TypeAnn::F64,
            "i32" | "int" | "u32" => TypeAnn::I32,
            "bool" => TypeAnn::Bool,
            "vec2" | "vec2f" => TypeAnn::Vec2,
            "vec3" | "vec3f" => TypeAnn::Vec3,
            "vec4" | "vec4f" => TypeAnn::Vec4,
            "mat4" | "mat4f" => TypeAnn::Mat4,
            _ => return Err(ShaderError::new(t.start, t.end, format!("unknown type `{}` (f32, f64, i32, bool, vec2, vec3, vec4, mat4)", name))),
        }))
    }

    /// `name: expr` pairs up to the closing `}` (already past `{`).
    fn fields(&mut self) -> PResult<Vec<(String, Expr)>> {
        let mut fields = Vec::new();
        loop {
            self.skip_seps();
            if self.eat("}") {
                return Ok(fields);
            }
            let name = self.ident()?;
            self.expect(":")?;
            let value = self.expr()?;
            fields.push((name, value));
        }
    }

    fn fn_decl(&mut self, start: usize) -> PResult<FnDecl> {
        let name = self.ident()?;
        self.expect("(")?;
        let mut params = Vec::new();
        loop {
            self.skip_seps();
            if self.eat(")") {
                break;
            }
            let p = self.ident()?;
            let ann = self.type_ann()?;
            params.push((p, ann));
            if !self.is(")") {
                self.expect(",")?;
            }
        }
        if self.eat("->") {
            // Return types are inferred; an annotation is checked loosely.
            self.ident()?;
        }
        let body = self.block()?;
        Ok(FnDecl { name, params, body, span: Span { start, end: self.prev_end() } })
    }

    fn block(&mut self) -> PResult<Vec<Stmt>> {
        self.expect("{")?;
        let mut stmts = Vec::new();
        loop {
            self.skip_seps();
            if self.eat("}") {
                return Ok(stmts);
            }
            if matches!(self.peek(), Tk::Eof) {
                return self.err("unclosed `{`");
            }
            stmts.push(self.stmt()?);
        }
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.tok().start;
        let span = |p: &Self| Span { start, end: p.prev_end() };
        if self.is_kw("let") || self.is_kw("var") {
            self.bump();
            let name = self.ident()?;
            let ann = self.type_ann()?;
            self.expect("=")?;
            let value = self.expr()?;
            return Ok(Stmt::Let { name, ann, value, span: span(self) });
        }
        if self.is_kw("for") {
            self.bump();
            let var = self.ident()?;
            if !self.is_kw("in") {
                return self.err("expected `in`");
            }
            self.bump();
            let from = self.expr_bp(0, true)?;
            self.expect("..")?;
            let to = self.expr_bp(0, true)?;
            let body = self.block()?;
            return Ok(Stmt::For { var, from, to, body, span: span(self) });
        }
        if self.is_kw("while") {
            self.bump();
            let cond = self.expr_bp(0, true)?;
            let body = self.block()?;
            return Ok(Stmt::While { cond, body, span: span(self) });
        }
        if self.is_kw("loop") {
            self.bump();
            let body = self.block()?;
            return Ok(Stmt::Loop { body, span: span(self) });
        }
        if self.is_kw("break") {
            self.bump();
            return Ok(Stmt::Break(span(self)));
        }
        if self.is_kw("continue") {
            self.bump();
            return Ok(Stmt::Continue(span(self)));
        }
        if self.is_kw("return") {
            self.bump();
            let value = if self.tok().nl || self.is("}") || self.is(";") {
                None
            } else {
                Some(self.expr()?)
            };
            return Ok(Stmt::Return(value, span(self)));
        }
        let e = self.expr()?;
        let op = match self.peek() {
            Tk::Punct("=") => Some(AssignOp::Set),
            Tk::Punct("+=") => Some(AssignOp::Add),
            Tk::Punct("-=") => Some(AssignOp::Sub),
            Tk::Punct("*=") => Some(AssignOp::Mul),
            Tk::Punct("/=") => Some(AssignOp::Div),
            Tk::Punct("%=") => Some(AssignOp::Rem),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let value = self.expr()?;
            return Ok(Stmt::Assign { target: e, op, value, span: span(self) });
        }
        Ok(Stmt::Expr(e))
    }

    pub fn expr(&mut self) -> PResult<Expr> {
        self.expr_bp(0, false)
    }

    fn binop(&self) -> Option<(BinOp, u8)> {
        if self.tok().nl {
            return None;
        }
        Some(match self.peek() {
            Tk::Punct("||") => (BinOp::Or, 1),
            Tk::Ident(s) if s == "or" => (BinOp::Or, 1),
            Tk::Punct("&&") => (BinOp::And, 2),
            Tk::Ident(s) if s == "and" => (BinOp::And, 2),
            Tk::Punct("==") => (BinOp::Eq, 3),
            Tk::Punct("!=") => (BinOp::Ne, 3),
            Tk::Punct("<") => (BinOp::Lt, 3),
            Tk::Punct("<=") => (BinOp::Le, 3),
            Tk::Punct(">") => (BinOp::Gt, 3),
            Tk::Punct(">=") => (BinOp::Ge, 3),
            Tk::Punct("|") => (BinOp::BitOr, 4),
            Tk::Punct("^") => (BinOp::BitXor, 5),
            Tk::Punct("&") => (BinOp::BitAnd, 6),
            Tk::Punct("<<") => (BinOp::Shl, 7),
            Tk::Punct(">>") => (BinOp::Shr, 7),
            Tk::Punct(">>>") => (BinOp::ShrU, 7),
            Tk::Punct("+") => (BinOp::Add, 8),
            Tk::Punct("-") => (BinOp::Sub, 8),
            Tk::Punct("*") => (BinOp::Mul, 9),
            Tk::Punct("/") => (BinOp::Div, 9),
            Tk::Punct("%") => (BinOp::Rem, 9),
            _ => return None,
        })
    }

    /// Precedence climbing. `no_struct`: in `if`/`while`/`for` headers an
    /// `Upper {` is the body, not a struct literal.
    fn expr_bp(&mut self, min: u8, no_struct: bool) -> PResult<Expr> {
        let mut lhs = self.unary(no_struct)?;
        while let Some((op, prec)) = self.binop() {
            if prec <= min {
                break;
            }
            self.bump();
            let rhs = self.expr_bp(prec, no_struct)?;
            let span = Span { start: lhs.span.start, end: rhs.span.end };
            lhs = Expr { kind: ExprKind::Bin(op, Box::new(lhs), Box::new(rhs)), span };
        }
        Ok(lhs)
    }

    fn unary(&mut self, no_struct: bool) -> PResult<Expr> {
        let start = self.tok().start;
        if self.eat("-") {
            let e = self.unary(no_struct)?;
            let span = Span { start, end: e.span.end };
            return Ok(Expr { kind: ExprKind::Neg(Box::new(e)), span });
        }
        if self.eat("!") || (self.is_kw("not") && { self.bump(); true }) {
            let e = self.unary(no_struct)?;
            let span = Span { start, end: e.span.end };
            return Ok(Expr { kind: ExprKind::Not(Box::new(e)), span });
        }
        let mut e = self.primary(no_struct)?;
        loop {
            if self.tok().nl {
                break;
            }
            if self.eat(".") {
                let name = match self.peek().clone() {
                    Tk::Num(v, true) => {
                        self.bump();
                        format!("{}", v as u64)
                    }
                    _ => self.ident()?,
                };
                // `module.fn(args)`: a qualified call.
                if self.is("(") && !self.tok().nl {
                    if let Some(q) = dotted(&e) {
                        self.bump();
                        let mut args = Vec::new();
                        loop {
                            self.skip_seps();
                            if self.eat(")") {
                                break;
                            }
                            args.push(self.expr()?);
                            if !self.is(")") && !self.is(",") {
                                self.expect(")")?;
                            }
                        }
                        let span = Span { start, end: self.prev_end() };
                        e = Expr { kind: ExprKind::Call(format!("{}.{}", q, name), args), span };
                        continue;
                    }
                }
                let span = Span { start, end: self.prev_end() };
                e = Expr { kind: ExprKind::Field(Box::new(e), name), span };
            } else if self.eat("[") {
                let idx = self.expr()?;
                self.expect("]")?;
                let span = Span { start, end: self.prev_end() };
                e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(idx)), span };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn primary(&mut self, no_struct: bool) -> PResult<Expr> {
        let t = self.tok().clone();
        let start = t.start;
        let done = |p: &Self, kind| Ok(Expr { kind, span: Span { start, end: p.prev_end() } });
        match t.tk {
            Tk::Num(v, int) => {
                self.bump();
                done(self, ExprKind::Num(v, int))
            }
            Tk::Color(c) => {
                // `vec4(r, g, b, a)`, each channel / 255 (what the VM and
                // shaders give a colour literal).
                self.bump();
                let span = Span { start, end: self.prev_end() };
                let ch = |k: u32| Expr { kind: ExprKind::Num(((c >> (24 - 8 * k)) & 0xff) as f64 / 255.0, false), span };
                done(self, ExprKind::Call("vec4".into(), vec![ch(0), ch(1), ch(2), ch(3)]))
            }
            Tk::Punct("(") => {
                self.bump();
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Tk::Punct("{") => {
                let b = self.block()?;
                done(self, ExprKind::Block(b))
            }
            Tk::Punct("[") => {
                self.bump();
                self.skip_seps();
                if self.eat("]") {
                    return self.err("empty array: give a size, `[0.0; 64]`");
                }
                let first = self.expr()?;
                if self.eat(";") {
                    let count = self.expr()?;
                    self.expect("]")?;
                    return done(self, ExprKind::ArrayRepeat(Box::new(first), Box::new(count)));
                }
                let mut list = vec![first];
                loop {
                    self.skip_seps();
                    if self.eat("]") {
                        break;
                    }
                    list.push(self.expr()?);
                }
                done(self, ExprKind::ArrayList(list))
            }
            Tk::Ident(name) => {
                match name.as_str() {
                    "true" | "false" => {
                        self.bump();
                        return done(self, ExprKind::Bool(name == "true"));
                    }
                    "if" => return self.if_expr(),
                    "match" => return self.match_expr(),
                    "let" | "var" | "fn" | "for" | "while" | "loop" | "return" | "break" | "continue" => {
                        return self.err(format!("`{}` cannot be used as a value here", name))
                    }
                    _ => {}
                }
                self.bump();
                if self.is("(") && !self.tok().nl {
                    self.bump();
                    let mut args = Vec::new();
                    loop {
                        self.skip_seps();
                        if self.eat(")") {
                            break;
                        }
                        args.push(self.expr()?);
                        if !self.is(")") && !self.is(",") {
                            self.expect(")")?;
                        }
                    }
                    return done(self, ExprKind::Call(name, args));
                }
                let upper = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
                if upper && !no_struct && self.is("{") {
                    self.bump();
                    let fields = self.fields()?;
                    return done(self, ExprKind::StructLit(name, fields));
                }
                done(self, ExprKind::Ident(name))
            }
            Tk::Str(_) => self.err("no strings here (a string only names a library: `use lib(\"id\", \"rev\")`)"),
            ref other => self.err(format!("expected a value, found {}", describe(other))),
        }
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.tok().start;
        self.bump();
        let mut arms = Vec::new();
        let cond = self.expr_bp(0, true)?;
        let body = self.block()?;
        arms.push((cond, body));
        let mut else_ = None;
        loop {
            if self.is_kw("elif") {
                self.bump();
                let cond = self.expr_bp(0, true)?;
                let body = self.block()?;
                arms.push((cond, body));
            } else if self.is_kw("else") {
                self.bump();
                if self.is_kw("if") {
                    self.bump();
                    let cond = self.expr_bp(0, true)?;
                    let body = self.block()?;
                    arms.push((cond, body));
                } else {
                    else_ = Some(self.block()?);
                    break;
                }
            } else {
                break;
            }
        }
        Ok(Expr { kind: ExprKind::If(arms, else_), span: Span { start, end: self.prev_end() } })
    }

    fn match_expr(&mut self) -> PResult<Expr> {
        let start = self.tok().start;
        self.bump();
        let subject = self.expr_bp(0, true)?;
        self.expect("{")?;
        let mut arms = Vec::new();
        loop {
            self.skip_seps();
            if self.eat("}") {
                break;
            }
            let pats = if matches!(self.peek(), Tk::Ident(s) if s == "_") {
                self.bump();
                None
            } else {
                let mut pats = vec![self.expr_bp(4, true)?];
                while self.eat("|") {
                    pats.push(self.expr_bp(4, true)?);
                }
                Some(pats)
            };
            self.expect("=>")?;
            let body = if self.is("{") {
                self.block()?
            } else {
                vec![Stmt::Expr(self.expr()?)]
            };
            arms.push((pats, body));
        }
        Ok(Expr { kind: ExprKind::Match(Box::new(subject), arms), span: Span { start, end: self.prev_end() } })
    }
}

fn describe(tk: &Tk) -> String {
    match tk {
        Tk::Ident(s) => format!("`{}`", s),
        Tk::Num(v, _) => format!("`{}`", v),
        Tk::Punct(p) => format!("`{}`", p),
        Tk::Str(_) => "a string".into(),
        Tk::Color(c) => format!("`#{:08x}`", c),
        Tk::Eof => "the end of the code".into(),
    }
}
