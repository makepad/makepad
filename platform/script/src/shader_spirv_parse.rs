//! The front of the SPIR-V backend: tokens and syntax tree of the shader
//! module text the draw-shader emitter writes (`shader_wgsl`), the language
//! WebGPU runs as is. Only the constructs that emitter (and the platform's
//! own fixed shaders) write are accepted; anything else is an error naming
//! its line.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tok {
    Ident,
    Int,
    Float,
    Punct,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: Tok,
    pub start: u32,
    pub end: u32,
    pub line: u32,
}

/// A type as written: a name with template arguments (`vec4<f32>`,
/// `array<u32, 4>`, `ptr<function, Sdf2d>`). Numbers and address spaces are
/// arguments by their text.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeAst {
    pub name: String,
    pub args: Vec<TypeAst>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LogicAnd,
    LogicOr,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

pub type ExprId = u32;

#[derive(Clone, Debug)]
pub enum Expr {
    Int(i64, u8),
    Float(f64, u8),
    Bool(bool),
    Ident(String),
    Call { name: String, templ: Vec<TypeAst>, args: Vec<ExprId> },
    Unary(UnOp, ExprId),
    Binary(BinOp, ExprId, ExprId),
    Member(ExprId, String),
    Index(ExprId, ExprId),
    AddrOf(ExprId),
    Deref(ExprId),
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let { name: String, ty: Option<TypeAst>, init: ExprId },
    Var { name: String, ty: Option<TypeAst>, init: Option<ExprId> },
    Assign { lhs: ExprId, op: Option<BinOp>, rhs: ExprId },
    Incr(ExprId, bool),
    Phony(ExprId),
    Call(ExprId),
    If { cond: ExprId, then: Vec<Stmt>, els: Vec<Stmt> },
    Loop { body: Vec<Stmt>, continuing: Vec<Stmt>, break_if: Option<ExprId> },
    For { init: Option<Box<Stmt>>, cond: Option<ExprId>, update: Option<Box<Stmt>>, body: Vec<Stmt> },
    While { cond: ExprId, body: Vec<Stmt> },
    Break,
    Continue,
    Return(Option<ExprId>),
    Discard,
    Block(Vec<Stmt>),
}

#[derive(Clone, Debug, Default)]
pub struct Attrs {
    pub group: Option<u32>,
    pub binding: Option<u32>,
    pub location: Option<u32>,
    pub builtin: Option<String>,
    pub flat: bool,
    pub invariant: bool,
    pub stage: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StructMemberAst {
    pub name: String,
    pub ty: TypeAst,
    pub attrs: Attrs,
}

#[derive(Clone, Debug)]
pub struct StructAst {
    pub name: String,
    pub members: Vec<StructMemberAst>,
}

#[derive(Clone, Debug)]
pub struct GlobalAst {
    pub name: String,
    /// `private`, `uniform`, `storage` or empty for a handle (texture,
    /// sampler); `const` for a module constant.
    pub space: String,
    pub write: bool,
    pub ty: Option<TypeAst>,
    pub init: Option<ExprId>,
    pub attrs: Attrs,
}

#[derive(Clone, Debug)]
pub struct ParamAst {
    pub name: String,
    pub ty: TypeAst,
    pub attrs: Attrs,
}

#[derive(Clone, Debug)]
pub struct FnAst {
    pub name: String,
    pub params: Vec<ParamAst>,
    pub ret: Option<TypeAst>,
    pub ret_attrs: Attrs,
    pub attrs: Attrs,
    pub body: Vec<Stmt>,
}

#[derive(Default, Debug)]
pub struct ModuleAst {
    pub exprs: Vec<Expr>,
    pub expr_lines: Vec<u32>,
    pub structs: Vec<StructAst>,
    pub globals: Vec<GlobalAst>,
    pub fns: Vec<FnAst>,
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let b = src.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        let start = i;
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            toks.push(Token { kind: Tok::Ident, start: start as u32, end: i as u32, line });
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let mut is_float = false;
            if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if i < b.len() && b[i] == b'.' {
                    is_float = true;
                    i += 1;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                    let mut j = i + 1;
                    if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                        j += 1;
                    }
                    if j < b.len() && b[j].is_ascii_digit() {
                        is_float = true;
                        i = j;
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
            }
            if i < b.len() && (b[i] == b'f' || b[i] == b'h') {
                is_float = true;
                i += 1;
            } else if i < b.len() && (b[i] == b'u' || b[i] == b'i') {
                i += 1;
            }
            let kind = if is_float { Tok::Float } else { Tok::Int };
            toks.push(Token { kind, start: start as u32, end: i as u32, line });
            continue;
        }
        let three = [b"<<=", b">>="];
        let two = [
            b"->", b"&&", b"||", b"==", b"!=", b"<=", b">=", b"<<", b">>", b"++", b"--", b"+=", b"-=",
            b"*=", b"/=", b"%=", b"&=", b"|=", b"^=",
        ];
        let mut len = 1;
        for p in three {
            if b[i..].starts_with(p) {
                len = 3;
            }
        }
        if len == 1 {
            for p in two {
                if b[i..].starts_with(p) {
                    len = 2;
                }
            }
        }
        if len == 1 && !b"(){}[]<>,;:.=+-*/%&|^!~@".contains(&c) {
            return Err(format!("line {}: unexpected character '{}'", line, c as char));
        }
        i += len;
        toks.push(Token { kind: Tok::Punct, start: start as u32, end: i as u32, line });
    }
    toks.push(Token { kind: Tok::Eof, start: b.len() as u32, end: b.len() as u32, line });
    Ok(toks)
}

pub struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    pub module: ModuleAst,
}

impl<'a> Parser<'a> {
    pub fn parse(src: &'a str) -> Result<ModuleAst, String> {
        let toks = tokenize(src)?;
        let mut p = Parser { src, toks, pos: 0, module: ModuleAst::default() };
        p.parse_module()?;
        Ok(p.module)
    }

    fn text(&self, t: Token) -> &'a str {
        &self.src[t.start as usize..t.end as usize]
    }
    fn peek(&self) -> Token {
        self.toks[self.pos]
    }
    fn peek_at(&self, n: usize) -> Token {
        let i = (self.pos + n).min(self.toks.len() - 1);
        self.toks[i]
    }
    fn is(&self, s: &str) -> bool {
        let t = self.peek();
        t.kind != Tok::Eof && self.text(t) == s
    }
    fn next(&mut self) -> Token {
        let t = self.toks[self.pos];
        if t.kind != Tok::Eof {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.is(s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn err<T>(&self, what: &str) -> Result<T, String> {
        let t = self.peek();
        Err(format!("line {}: {} (found '{}')", t.line, what, self.text(t)))
    }
    fn expect(&mut self, s: &str) -> Result<(), String> {
        if self.eat(s) {
            Ok(())
        } else {
            self.err(&format!("expected '{}'", s))
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        let t = self.peek();
        if t.kind != Tok::Ident {
            return self.err("expected an identifier");
        }
        self.pos += 1;
        Ok(self.text(t).to_string())
    }

    /// Splits a `>>` or `>=` closing a template so its first `>` closes it.
    fn expect_template_close(&mut self) -> Result<(), String> {
        let t = self.peek();
        if t.kind == Tok::Punct {
            let s = self.text(t);
            if s == ">" {
                self.pos += 1;
                return Ok(());
            }
            if s == ">>" || s == ">=" || s == ">>=" {
                self.toks[self.pos].start += 1;
                return Ok(());
            }
        }
        self.err("expected '>'")
    }

    fn parse_type(&mut self) -> Result<TypeAst, String> {
        let t = self.peek();
        let name = if t.kind == Tok::Int {
            self.pos += 1;
            self.text(t).to_string()
        } else {
            self.ident()?
        };
        let mut args = Vec::new();
        if self.is("<") {
            self.pos += 1;
            loop {
                args.push(self.parse_type()?);
                if !self.eat(",") {
                    break;
                }
            }
            self.expect_template_close()?;
        }
        Ok(TypeAst { name, args })
    }

    fn parse_attrs(&mut self) -> Result<Attrs, String> {
        let mut a = Attrs::default();
        while self.eat("@") {
            let name = self.ident()?;
            let mut args: Vec<String> = Vec::new();
            if self.eat("(") {
                while !self.is(")") {
                    let t = self.next();
                    if t.kind == Tok::Eof {
                        return self.err("unterminated attribute");
                    }
                    if self.text(t) != "," {
                        args.push(self.text(t).to_string());
                    }
                }
                self.expect(")")?;
            }
            let num = |args: &Vec<String>| -> Result<u32, String> {
                let s = args.first().map(|s| s.trim_end_matches(['u', 'i'])).unwrap_or("");
                s.parse::<u32>().map_err(|_| format!("bad attribute argument for @{}", name))
            };
            match name.as_str() {
                "group" => a.group = Some(num(&args)?),
                "binding" => a.binding = Some(num(&args)?),
                "location" => a.location = Some(num(&args)?),
                "builtin" => a.builtin = args.first().cloned(),
                "interpolate" => a.flat = args.first().map(|s| s == "flat").unwrap_or(false),
                "invariant" => a.invariant = true,
                "vertex" | "fragment" | "compute" => a.stage = Some(name.clone()),
                "must_use" | "align" | "size" | "id" | "workgroup_size" => {
                    if name == "align" || name == "size" {
                        return self.err("@align/@size are not supported");
                    }
                }
                _ => return self.err(&format!("unknown attribute @{}", name)),
            }
        }
        Ok(a)
    }

    fn parse_module(&mut self) -> Result<(), String> {
        loop {
            if self.peek().kind == Tok::Eof {
                return Ok(());
            }
            if self.eat(";") {
                continue;
            }
            if self.is("enable") || self.is("requires") || self.is("diagnostic") {
                while !self.eat(";") {
                    if self.next().kind == Tok::Eof {
                        return Ok(());
                    }
                }
                continue;
            }
            let attrs = self.parse_attrs()?;
            if self.eat("struct") {
                let name = self.ident()?;
                self.expect("{")?;
                let mut members = Vec::new();
                while !self.eat("}") {
                    let attrs = self.parse_attrs()?;
                    let name = self.ident()?;
                    self.expect(":")?;
                    let ty = self.parse_type()?;
                    members.push(StructMemberAst { name, ty, attrs });
                    if !self.eat(",") && !self.eat(";") && !self.is("}") {
                        return self.err("expected ',' in struct");
                    }
                }
                self.module.structs.push(StructAst { name, members });
            } else if self.eat("var") {
                let mut space = String::new();
                let mut write = false;
                if self.eat("<") {
                    space = self.ident()?;
                    if self.eat(",") {
                        let access = self.ident()?;
                        write = access == "read_write" || access == "write";
                    } else if space == "storage" {
                        write = false;
                    }
                    self.expect_template_close()?;
                }
                let name = self.ident()?;
                let ty = if self.eat(":") { Some(self.parse_type()?) } else { None };
                let init = if self.eat("=") { Some(self.parse_expr()?) } else { None };
                self.expect(";")?;
                self.module.globals.push(GlobalAst { name, space, write, ty, init, attrs });
            } else if self.is("const") || self.is("override") || self.is("let") {
                self.pos += 1;
                let name = self.ident()?;
                let ty = if self.eat(":") { Some(self.parse_type()?) } else { None };
                self.expect("=")?;
                let init = Some(self.parse_expr()?);
                self.expect(";")?;
                self.module.globals.push(GlobalAst {
                    name,
                    space: "const".to_string(),
                    write: false,
                    ty,
                    init,
                    attrs,
                });
            } else if self.eat("fn") {
                let name = self.ident()?;
                self.expect("(")?;
                let mut params = Vec::new();
                while !self.eat(")") {
                    let attrs = self.parse_attrs()?;
                    let name = self.ident()?;
                    self.expect(":")?;
                    let ty = self.parse_type()?;
                    params.push(ParamAst { name, ty, attrs });
                    if !self.eat(",") && !self.is(")") {
                        return self.err("expected ',' in parameters");
                    }
                }
                let mut ret = None;
                let mut ret_attrs = Attrs::default();
                if self.eat("->") {
                    ret_attrs = self.parse_attrs()?;
                    ret = Some(self.parse_type()?);
                }
                let body = self.parse_block()?;
                self.module.fns.push(FnAst { name, params, ret, ret_attrs, attrs, body });
            } else {
                return self.err("expected a module declaration");
            }
        }
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect("{")?;
        let mut stmts = Vec::new();
        while !self.eat("}") {
            if self.peek().kind == Tok::Eof {
                return self.err("unterminated block");
            }
            if let Some(s) = self.parse_stmt()? {
                stmts.push(s);
            }
        }
        Ok(stmts)
    }

    /// A statement, or None for an empty one (`;`).
    fn parse_stmt(&mut self) -> Result<Option<Stmt>, String> {
        if self.eat(";") {
            return Ok(None);
        }
        if self.is("{") {
            return Ok(Some(Stmt::Block(self.parse_block()?)));
        }
        if self.eat("if") {
            return Ok(Some(self.parse_if()?));
        }
        if self.eat("loop") {
            self.expect("{")?;
            let mut body = Vec::new();
            let mut continuing = Vec::new();
            let mut break_if = None;
            while !self.eat("}") {
                if self.eat("continuing") {
                    self.expect("{")?;
                    while !self.eat("}") {
                        if self.is("break") && self.peek_at(1).kind == Tok::Ident && self.text(self.peek_at(1)) == "if" {
                            self.pos += 2;
                            break_if = Some(self.parse_expr()?);
                            self.expect(";")?;
                            continue;
                        }
                        if let Some(s) = self.parse_stmt()? {
                            continuing.push(s);
                        }
                    }
                    continue;
                }
                if self.peek().kind == Tok::Eof {
                    return self.err("unterminated loop");
                }
                if let Some(s) = self.parse_stmt()? {
                    body.push(s);
                }
            }
            return Ok(Some(Stmt::Loop { body, continuing, break_if }));
        }
        if self.eat("for") {
            self.expect("(")?;
            let init = if self.is(";") { None } else { self.parse_simple_stmt()?.map(Box::new) };
            self.expect(";")?;
            let cond = if self.is(";") { None } else { Some(self.parse_expr()?) };
            self.expect(";")?;
            let update = if self.is(")") { None } else { self.parse_simple_stmt()?.map(Box::new) };
            self.expect(")")?;
            let body = self.parse_block()?;
            return Ok(Some(Stmt::For { init, cond, update, body }));
        }
        if self.eat("while") {
            let cond = self.parse_expr()?;
            let body = self.parse_block()?;
            return Ok(Some(Stmt::While { cond, body }));
        }
        if self.eat("break") {
            self.expect(";")?;
            return Ok(Some(Stmt::Break));
        }
        if self.eat("continue") {
            self.expect(";")?;
            return Ok(Some(Stmt::Continue));
        }
        if self.eat("discard") {
            self.expect(";")?;
            return Ok(Some(Stmt::Discard));
        }
        if self.eat("return") {
            let value = if self.is(";") { None } else { Some(self.parse_expr()?) };
            self.expect(";")?;
            return Ok(Some(Stmt::Return(value)));
        }
        let s = self.parse_simple_stmt()?;
        self.expect(";")?;
        Ok(s)
    }

    fn parse_if(&mut self) -> Result<Stmt, String> {
        let cond = self.parse_expr()?;
        let then = self.parse_block()?;
        let mut els = Vec::new();
        if self.eat("else") {
            if self.eat("if") {
                els.push(self.parse_if()?);
            } else {
                els = self.parse_block()?;
            }
        }
        Ok(Stmt::If { cond, then, els })
    }

    /// Declarations, assignments, increments and calls: the statements a
    /// `for` header may hold.
    fn parse_simple_stmt(&mut self) -> Result<Option<Stmt>, String> {
        if self.eat("let") || self.eat("const") {
            let name = self.ident()?;
            let ty = if self.eat(":") { Some(self.parse_type()?) } else { None };
            self.expect("=")?;
            let init = self.parse_expr()?;
            return Ok(Some(Stmt::Let { name, ty, init }));
        }
        if self.eat("var") {
            if self.eat("<") {
                // `var<function>`
                self.ident()?;
                self.expect_template_close()?;
            }
            let name = self.ident()?;
            let ty = if self.eat(":") { Some(self.parse_type()?) } else { None };
            let init = if self.eat("=") { Some(self.parse_expr()?) } else { None };
            return Ok(Some(Stmt::Var { name, ty, init }));
        }
        if self.is("_") && self.peek_at(1).kind == Tok::Punct && self.text(self.peek_at(1)) == "=" {
            self.pos += 2;
            let e = self.parse_expr()?;
            return Ok(Some(Stmt::Phony(e)));
        }
        let lhs = self.parse_unary()?;
        let t = self.peek();
        if t.kind == Tok::Punct {
            let op = match self.text(t) {
                "=" => Some(None),
                "+=" => Some(Some(BinOp::Add)),
                "-=" => Some(Some(BinOp::Sub)),
                "*=" => Some(Some(BinOp::Mul)),
                "/=" => Some(Some(BinOp::Div)),
                "%=" => Some(Some(BinOp::Rem)),
                "&=" => Some(Some(BinOp::BitAnd)),
                "|=" => Some(Some(BinOp::BitOr)),
                "^=" => Some(Some(BinOp::BitXor)),
                "<<=" => Some(Some(BinOp::Shl)),
                ">>=" => Some(Some(BinOp::Shr)),
                _ => None,
            };
            if let Some(op) = op {
                self.pos += 1;
                let rhs = self.parse_expr()?;
                return Ok(Some(Stmt::Assign { lhs, op, rhs }));
            }
            if self.text(t) == "++" || self.text(t) == "--" {
                self.pos += 1;
                return Ok(Some(Stmt::Incr(lhs, self.text(t) == "++")));
            }
        }
        match &self.module.exprs[lhs as usize] {
            Expr::Call { .. } => Ok(Some(Stmt::Call(lhs))),
            _ => self.err("expected an assignment or a call"),
        }
    }

    fn push(&mut self, e: Expr, line: u32) -> ExprId {
        self.module.exprs.push(e);
        self.module.expr_lines.push(line);
        (self.module.exprs.len() - 1) as ExprId
    }

    pub fn parse_expr(&mut self) -> Result<ExprId, String> {
        self.parse_binary(0)
    }

    fn binop(&self) -> Option<(BinOp, u32)> {
        let t = self.peek();
        if t.kind != Tok::Punct {
            return None;
        }
        // WGSL has no precedence between the logical/bitwise groups and
        // requires parentheses there; these levels order what remains.
        Some(match self.text(t) {
            "||" => (BinOp::LogicOr, 1),
            "&&" => (BinOp::LogicAnd, 2),
            "|" => (BinOp::BitOr, 3),
            "^" => (BinOp::BitXor, 4),
            "&" => (BinOp::BitAnd, 5),
            "==" => (BinOp::Eq, 6),
            "!=" => (BinOp::Ne, 6),
            "<" => (BinOp::Lt, 7),
            "<=" => (BinOp::Le, 7),
            ">" => (BinOp::Gt, 7),
            ">=" => (BinOp::Ge, 7),
            "<<" => (BinOp::Shl, 8),
            ">>" => (BinOp::Shr, 8),
            "+" => (BinOp::Add, 9),
            "-" => (BinOp::Sub, 9),
            "*" => (BinOp::Mul, 10),
            "/" => (BinOp::Div, 10),
            "%" => (BinOp::Rem, 10),
            _ => return None,
        })
    }

    fn parse_binary(&mut self, min_prec: u32) -> Result<ExprId, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            let Some((op, prec)) = self.binop() else { break };
            if prec <= min_prec {
                break;
            }
            let line = self.peek().line;
            self.pos += 1;
            let rhs = self.parse_binary(prec)?;
            lhs = self.push(Expr::Binary(op, lhs, rhs), line);
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<ExprId, String> {
        let t = self.peek();
        if t.kind == Tok::Punct {
            let op = match self.text(t) {
                "-" => Some(UnOp::Neg),
                "!" => Some(UnOp::Not),
                "~" => Some(UnOp::BitNot),
                _ => None,
            };
            if let Some(op) = op {
                self.pos += 1;
                let e = self.parse_unary()?;
                return Ok(self.push(Expr::Unary(op, e), t.line));
            }
            if self.text(t) == "&" {
                self.pos += 1;
                let e = self.parse_unary()?;
                return Ok(self.push(Expr::AddrOf(e), t.line));
            }
            if self.text(t) == "*" {
                self.pos += 1;
                let e = self.parse_unary()?;
                return Ok(self.push(Expr::Deref(e), t.line));
            }
        }
        let mut e = self.parse_primary()?;
        loop {
            let line = self.peek().line;
            if self.eat(".") {
                let name = self.ident()?;
                e = self.push(Expr::Member(e, name), line);
            } else if self.eat("[") {
                let index = self.parse_expr()?;
                self.expect("]")?;
                e = self.push(Expr::Index(e, index), line);
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> Result<ExprId, String> {
        let t = self.peek();
        match t.kind {
            Tok::Int => {
                self.pos += 1;
                let s = self.text(t);
                let (digits, suffix) = match s.as_bytes()[s.len() - 1] {
                    b'u' => (&s[..s.len() - 1], b'u'),
                    b'i' => (&s[..s.len() - 1], b'i'),
                    _ => (s, 0),
                };
                let v = if let Some(hex) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
                    i64::from_str_radix(hex, 16)
                } else {
                    digits.parse::<i64>()
                };
                let v = v.map_err(|_| format!("line {}: bad integer '{}'", t.line, s))?;
                Ok(self.push(Expr::Int(v, suffix), t.line))
            }
            Tok::Float => {
                self.pos += 1;
                let s = self.text(t);
                let (digits, suffix) = match s.as_bytes()[s.len() - 1] {
                    b'f' => (&s[..s.len() - 1], b'f'),
                    b'h' => (&s[..s.len() - 1], b'h'),
                    _ => (s, 0),
                };
                let v = digits
                    .parse::<f64>()
                    .map_err(|_| format!("line {}: bad float '{}'", t.line, s))?;
                Ok(self.push(Expr::Float(v, suffix), t.line))
            }
            Tok::Punct if self.text(t) == "(" => {
                self.pos += 1;
                let e = self.parse_expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Ident => {
                let name = self.text(t);
                self.pos += 1;
                if name == "true" || name == "false" {
                    return Ok(self.push(Expr::Bool(name == "true"), t.line));
                }
                let mut templ = Vec::new();
                // A template list follows a type or `bitcast` written as a
                // callee: `vec4<f32>(..)`, `array<u32, 4>(..)`, `bitcast<u32>(..)`.
                if self.is("<") && templated_callee(name) {
                    self.pos += 1;
                    loop {
                        templ.push(self.parse_type()?);
                        if !self.eat(",") {
                            break;
                        }
                    }
                    self.expect_template_close()?;
                }
                if self.eat("(") {
                    let mut args = Vec::new();
                    while !self.eat(")") {
                        args.push(self.parse_expr()?);
                        if !self.eat(",") && !self.is(")") {
                            return self.err("expected ',' in arguments");
                        }
                    }
                    return Ok(self.push(Expr::Call { name: name.to_string(), templ, args }, t.line));
                }
                if !templ.is_empty() {
                    return self.err("expected '(' after a template list");
                }
                Ok(self.push(Expr::Ident(name.to_string()), t.line))
            }
            _ => self.err("expected an expression"),
        }
    }
}

fn templated_callee(name: &str) -> bool {
    matches!(
        name,
        "vec2" | "vec3" | "vec4" | "mat2x2" | "mat2x3" | "mat2x4" | "mat3x2" | "mat3x3" | "mat3x4"
            | "mat4x2" | "mat4x3" | "mat4x4" | "array" | "bitcast"
    )
}
