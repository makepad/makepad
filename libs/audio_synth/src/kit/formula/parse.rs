//! Lexer and recursive-descent parser for the formula language (the whole
//! grammar of the dialect: statements, operator precedence, long strings
//! and comments, string escapes, method calls and string/table call
//! sugar).

use super::ast::*;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Name(Rc<str>),
    Num(f64),
    Str(Rc<str>),
    /// Keywords and symbols.
    Sym(&'static str),
    Eof,
}

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Longest first, so `...` wins over `..` and `.`.
const SYMBOLS: &[&str] = &[
    "...", "..", "==", "~=", "<=", ">=", "::", "+", "-", "*", "/", "%", "^", "#", "<", ">", "=", "(", ")", "{", "}", "[", "]", ";", ":", ",", ".",
];

#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: u32,
    pub msg: String,
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
}

impl<'a> Lexer<'a> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError { line: self.line, msg: msg.into() })
    }

    fn peek(&self, k: usize) -> u8 {
        self.src.get(self.pos + k).copied().unwrap_or(0)
    }

    /// `[[`, `[=[` ...: the level, or None when this is not a long bracket.
    fn long_open(&self) -> Option<usize> {
        if self.peek(0) != b'[' {
            return None;
        }
        let mut k = 1;
        while self.peek(k) == b'=' {
            k += 1;
        }
        (self.peek(k) == b'[').then_some(k - 1)
    }

    fn long_body(&mut self, level: usize) -> Result<String, ParseError> {
        self.pos += level + 2;
        // A newline right after the opening bracket is skipped.
        if self.peek(0) == b'\r' {
            self.pos += 1;
        }
        if self.peek(0) == b'\n' {
            self.pos += 1;
            self.line += 1;
        }
        let start = self.pos;
        loop {
            if self.pos >= self.src.len() {
                return self.err("unfinished long string or comment");
            }
            if self.peek(0) == b']' {
                let mut k = 1;
                while self.peek(k) == b'=' {
                    k += 1;
                }
                if k - 1 == level && self.peek(k) == b']' {
                    let s = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                    self.pos += k + 1;
                    return Ok(s);
                }
            }
            if self.peek(0) == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }
    }

    fn skip_space(&mut self) -> Result<(), ParseError> {
        loop {
            let c = self.peek(0);
            if c == b'\n' {
                self.line += 1;
                self.pos += 1;
            } else if c == b' ' || c == b'\t' || c == b'\r' || c == 0x0b || c == 0x0c {
                self.pos += 1;
            } else if c == b'-' && self.peek(1) == b'-' {
                self.pos += 2;
                if let Some(level) = self.long_open() {
                    self.long_body(level)?;
                } else {
                    while self.pos < self.src.len() && self.peek(0) != b'\n' {
                        self.pos += 1;
                    }
                }
            } else if c == b'#' && self.pos == 0 && self.peek(1) == b'!' {
                while self.pos < self.src.len() && self.peek(0) != b'\n' {
                    self.pos += 1;
                }
            } else {
                return Ok(());
            }
        }
    }

    fn number(&mut self) -> Result<f64, ParseError> {
        let start = self.pos;
        if self.peek(0) == b'0' && (self.peek(1) == b'x' || self.peek(1) == b'X') {
            self.pos += 2;
            let mut v = 0f64;
            let mut any = false;
            while self.peek(0).is_ascii_hexdigit() {
                v = v * 16.0 + (self.peek(0) as char).to_digit(16).unwrap() as f64;
                self.pos += 1;
                any = true;
            }
            if !any {
                return self.err("malformed number");
            }
            return Ok(v);
        }
        while self.peek(0).is_ascii_digit() || self.peek(0) == b'.' {
            self.pos += 1;
        }
        if self.peek(0) == b'e' || self.peek(0) == b'E' {
            self.pos += 1;
            if self.peek(0) == b'+' || self.peek(0) == b'-' {
                self.pos += 1;
            }
            while self.peek(0).is_ascii_digit() {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
        if self.peek(0).is_ascii_alphabetic() || self.peek(0) == b'_' {
            return self.err(format!("malformed number near '{}'", text));
        }
        match text.parse::<f64>() {
            Ok(v) => Ok(v),
            Err(_) => self.err(format!("malformed number '{}'", text)),
        }
    }

    fn string(&mut self, q: u8) -> Result<String, ParseError> {
        self.pos += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = self.peek(0);
            if self.pos >= self.src.len() || c == b'\n' {
                return self.err("unfinished string");
            }
            self.pos += 1;
            if c == q {
                break;
            }
            if c != b'\\' {
                out.push(c);
                continue;
            }
            let e = self.peek(0);
            self.pos += 1;
            match e {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'a' => out.push(7),
                b'b' => out.push(8),
                b'f' => out.push(12),
                b'v' => out.push(11),
                b'\\' => out.push(b'\\'),
                b'"' => out.push(b'"'),
                b'\'' => out.push(b'\''),
                b'\n' => {
                    self.line += 1;
                    out.push(b'\n')
                }
                b'0'..=b'9' => {
                    let mut v = (e - b'0') as u32;
                    for _ in 0..2 {
                        if self.peek(0).is_ascii_digit() {
                            v = v * 10 + (self.peek(0) - b'0') as u32;
                            self.pos += 1;
                        }
                    }
                    if v > 255 {
                        return self.err("escape sequence too large");
                    }
                    out.push(v as u8);
                }
                _ => return self.err("invalid escape sequence"),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn next(&mut self) -> Result<(Tok, u32), ParseError> {
        self.skip_space()?;
        let line = self.line;
        let c = self.peek(0);
        if self.pos >= self.src.len() {
            return Ok((Tok::Eof, line));
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = self.pos;
            while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
                self.pos += 1;
            }
            let w = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
            if let Some(k) = KEYWORDS.iter().find(|k| **k == w) {
                return Ok((Tok::Sym(k), line));
            }
            return Ok((Tok::Name(w.into()), line));
        }
        if c.is_ascii_digit() || (c == b'.' && self.peek(1).is_ascii_digit()) {
            return Ok((Tok::Num(self.number()?), line));
        }
        if c == b'"' || c == b'\'' {
            return Ok((Tok::Str(self.string(c)?.into()), line));
        }
        if let Some(level) = self.long_open() {
            return Ok((Tok::Str(self.long_body(level)?.into()), line));
        }
        for s in SYMBOLS {
            if self.src[self.pos..].starts_with(s.as_bytes()) {
                self.pos += s.len();
                return Ok((Tok::Sym(s), line));
            }
        }
        self.err(format!("unexpected symbol '{}'", c as char))
    }
}

struct Parser {
    toks: Vec<(Tok, u32)>,
    at: usize,
}

type PR<T> = Result<T, ParseError>;

impl Parser {
    fn tok(&self) -> &Tok {
        &self.toks[self.at].0
    }
    fn line(&self) -> u32 {
        self.toks[self.at].1
    }
    fn peek_tok(&self, k: usize) -> &Tok {
        &self.toks[(self.at + k).min(self.toks.len() - 1)].0
    }
    fn is(&self, s: &str) -> bool {
        matches!(self.tok(), Tok::Sym(x) if *x == s)
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.is(s) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn err<T>(&self, msg: impl Into<String>) -> PR<T> {
        let near = match self.tok() {
            Tok::Name(n) => format!(" near '{}'", n),
            Tok::Sym(s) => format!(" near '{}'", s),
            Tok::Num(v) => format!(" near '{}'", v),
            Tok::Str(_) => " near a string".to_string(),
            Tok::Eof => " at the end".to_string(),
        };
        Err(ParseError { line: self.line(), msg: format!("{}{}", msg.into(), near) })
    }
    fn expect(&mut self, s: &str) -> PR<()> {
        if self.eat(s) {
            Ok(())
        } else {
            self.err(format!("'{}' expected", s))
        }
    }
    fn name(&mut self) -> PR<Rc<str>> {
        if let Tok::Name(n) = self.tok().clone() {
            self.at += 1;
            Ok(n)
        } else {
            self.err("name expected")
        }
    }

    fn block_end(&self) -> bool {
        matches!(self.tok(), Tok::Eof) || self.is("end") || self.is("else") || self.is("elseif") || self.is("until")
    }

    fn block(&mut self) -> PR<Block> {
        let mut out = Vec::new();
        loop {
            while self.eat(";") {}
            if self.block_end() {
                break;
            }
            let line = self.line();
            if self.eat("return") {
                let mut vals = Vec::new();
                if !self.block_end() && !self.is(";") {
                    vals = self.exprlist()?;
                }
                self.eat(";");
                out.push(Stat { kind: StatKind::Return(vals), line });
                if !self.block_end() {
                    return self.err("'end' expected after return");
                }
                break;
            }
            if self.eat("break") {
                out.push(Stat { kind: StatKind::Break, line });
                continue;
            }
            out.push(self.stat()?);
        }
        Ok(out)
    }

    fn stat(&mut self) -> PR<Stat> {
        let line = self.line();
        let kind = if self.eat("do") {
            let b = self.block()?;
            self.expect("end")?;
            StatKind::Do(b)
        } else if self.eat("while") {
            let c = self.expr()?;
            self.expect("do")?;
            let b = self.block()?;
            self.expect("end")?;
            StatKind::While(c, b)
        } else if self.eat("repeat") {
            let b = self.block()?;
            self.expect("until")?;
            let c = self.expr()?;
            StatKind::Repeat(b, c)
        } else if self.eat("if") {
            let mut arms = Vec::new();
            let c = self.expr()?;
            self.expect("then")?;
            arms.push((c, self.block()?));
            let mut els = None;
            loop {
                if self.eat("elseif") {
                    let c = self.expr()?;
                    self.expect("then")?;
                    arms.push((c, self.block()?));
                } else if self.eat("else") {
                    els = Some(self.block()?);
                    self.expect("end")?;
                    break;
                } else {
                    self.expect("end")?;
                    break;
                }
            }
            StatKind::If(arms, els)
        } else if self.eat("for") {
            let n1 = self.name()?;
            if self.eat("=") {
                let a = self.expr()?;
                self.expect(",")?;
                let b = self.expr()?;
                let step = if self.eat(",") { Some(self.expr()?) } else { None };
                self.expect("do")?;
                let body = self.block()?;
                self.expect("end")?;
                StatKind::NumFor(n1, a, b, step, body)
            } else {
                let mut names = vec![n1];
                while self.eat(",") {
                    names.push(self.name()?);
                }
                self.expect("in")?;
                let exprs = self.exprlist()?;
                self.expect("do")?;
                let body = self.block()?;
                self.expect("end")?;
                StatKind::GenFor(names, exprs, body)
            }
        } else if self.eat("function") {
            // funcname: Name {'.' Name} [':' Name]
            let first = self.name()?;
            let mut target = Expr { kind: ExprKind::Name(first.clone()), line };
            let mut full = first.to_string();
            let mut method = false;
            loop {
                if self.eat(".") {
                    let n = self.name()?;
                    full.push('.');
                    full.push_str(&n);
                    target = Expr { kind: ExprKind::Index(Box::new(target), Box::new(Expr { kind: ExprKind::Str(n), line })), line };
                } else if self.eat(":") {
                    let n = self.name()?;
                    full.push(':');
                    full.push_str(&n);
                    target = Expr { kind: ExprKind::Index(Box::new(target), Box::new(Expr { kind: ExprKind::Str(n), line })), line };
                    method = true;
                    break;
                } else {
                    break;
                }
            }
            let mut f = self.funcbody(line)?;
            if method {
                f.params.insert(0, "self".into());
            }
            f.name = Some(full.into());
            StatKind::Assign(vec![target], vec![Expr { kind: ExprKind::Func(Rc::new(f)), line }])
        } else if self.eat("local") {
            if self.eat("function") {
                let n = self.name()?;
                let mut f = self.funcbody(line)?;
                f.name = Some(n.clone());
                StatKind::LocalFunc(n, Rc::new(f))
            } else {
                let mut names = vec![self.name()?];
                // LuaJIT has no attribs; accept none.
                while self.eat(",") {
                    names.push(self.name()?);
                }
                let vals = if self.eat("=") { self.exprlist()? } else { Vec::new() };
                StatKind::Local(names, vals)
            }
        } else if self.is("goto") || self.is("::") {
            return self.err("goto and labels are not supported");
        } else {
            let e = self.suffixed()?;
            if self.is("=") || self.is(",") {
                let mut targets = vec![e];
                while self.eat(",") {
                    targets.push(self.suffixed()?);
                }
                self.expect("=")?;
                for t in &targets {
                    if !matches!(t.kind, ExprKind::Name(_) | ExprKind::Index(..)) {
                        return Err(ParseError { line: t.line, msg: "cannot assign to this expression".into() });
                    }
                }
                let vals = self.exprlist()?;
                StatKind::Assign(targets, vals)
            } else {
                if !matches!(e.kind, ExprKind::Call(..) | ExprKind::Method(..)) {
                    return self.err("syntax error (a statement must be a call or an assignment)");
                }
                StatKind::Call(e)
            }
        };
        Ok(Stat { kind, line })
    }

    fn funcbody(&mut self, line: u32) -> PR<FuncBody> {
        self.expect("(")?;
        let mut params = Vec::new();
        let mut vararg = false;
        if !self.is(")") {
            loop {
                if self.eat("...") {
                    vararg = true;
                    break;
                }
                params.push(self.name()?);
                if !self.eat(",") {
                    break;
                }
            }
        }
        self.expect(")")?;
        let body = self.block()?;
        self.expect("end")?;
        Ok(FuncBody { params, vararg, body, line, name: None })
    }

    fn exprlist(&mut self) -> PR<Vec<Expr>> {
        let mut v = vec![self.expr()?];
        while self.eat(",") {
            v.push(self.expr()?);
        }
        Ok(v)
    }

    fn primary(&mut self) -> PR<Expr> {
        let line = self.line();
        if let Tok::Name(n) = self.tok().clone() {
            self.at += 1;
            return Ok(Expr { kind: ExprKind::Name(n), line });
        }
        if self.eat("(") {
            let e = self.expr()?;
            self.expect(")")?;
            return Ok(Expr { kind: ExprKind::Paren(Box::new(e)), line });
        }
        self.err("unexpected symbol")
    }

    fn suffixed(&mut self) -> PR<Expr> {
        let mut e = self.primary()?;
        loop {
            let line = self.line();
            if self.eat(".") {
                let n = self.name()?;
                e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(Expr { kind: ExprKind::Str(n), line })), line };
            } else if self.eat("[") {
                let k = self.expr()?;
                self.expect("]")?;
                e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(k)), line };
            } else if self.eat(":") {
                let n = self.name()?;
                let args = self.callargs()?;
                e = Expr { kind: ExprKind::Method(Box::new(e), n, args), line };
            } else if self.is("(") || self.is("{") || matches!(self.tok(), Tok::Str(_)) {
                let args = self.callargs()?;
                e = Expr { kind: ExprKind::Call(Box::new(e), args), line };
            } else {
                return Ok(e);
            }
        }
    }

    fn callargs(&mut self) -> PR<Vec<Expr>> {
        let line = self.line();
        if let Tok::Str(s) = self.tok().clone() {
            self.at += 1;
            return Ok(vec![Expr { kind: ExprKind::Str(s), line }]);
        }
        if self.is("{") {
            return Ok(vec![self.table()?]);
        }
        self.expect("(")?;
        if self.eat(")") {
            return Ok(Vec::new());
        }
        let v = self.exprlist()?;
        self.expect(")")?;
        Ok(v)
    }

    fn table(&mut self) -> PR<Expr> {
        let line = self.line();
        self.expect("{")?;
        let mut fields = Vec::new();
        while !self.is("}") {
            if self.eat("[") {
                let k = self.expr()?;
                self.expect("]")?;
                self.expect("=")?;
                fields.push(Field::Keyed(k, self.expr()?));
            } else if matches!(self.tok(), Tok::Name(_)) && matches!(self.peek_tok(1), Tok::Sym("=")) {
                let kl = self.line();
                let n = self.name()?;
                self.expect("=")?;
                fields.push(Field::Keyed(Expr { kind: ExprKind::Str(n), line: kl }, self.expr()?));
            } else {
                fields.push(Field::Pos(self.expr()?));
            }
            if !self.eat(",") && !self.eat(";") {
                break;
            }
        }
        self.expect("}")?;
        Ok(Expr { kind: ExprKind::Table(fields), line })
    }

    fn simple(&mut self) -> PR<Expr> {
        let line = self.line();
        let kind = match self.tok().clone() {
            Tok::Num(v) => ExprKind::Num(v),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::Sym("nil") => ExprKind::Nil,
            Tok::Sym("true") => ExprKind::True,
            Tok::Sym("false") => ExprKind::False,
            Tok::Sym("...") => ExprKind::Vararg,
            Tok::Sym("{") => return self.table(),
            Tok::Sym("function") => {
                self.at += 1;
                return Ok(Expr { kind: ExprKind::Func(Rc::new(self.funcbody(line)?)), line });
            }
            _ => return self.suffixed(),
        };
        self.at += 1;
        Ok(Expr { kind, line })
    }

    /// Binary precedence (left, right) as the dialect defines it.
    fn binop(&self) -> Option<(BinOp, u8, u8)> {
        let s = match self.tok() {
            Tok::Sym(s) => *s,
            _ => return None,
        };
        Some(match s {
            "or" => (BinOp::Or, 1, 1),
            "and" => (BinOp::And, 2, 2),
            "<" => (BinOp::Lt, 3, 3),
            ">" => (BinOp::Gt, 3, 3),
            "<=" => (BinOp::Le, 3, 3),
            ">=" => (BinOp::Ge, 3, 3),
            "~=" => (BinOp::Ne, 3, 3),
            "==" => (BinOp::Eq, 3, 3),
            ".." => (BinOp::Concat, 5, 4),
            "+" => (BinOp::Add, 6, 6),
            "-" => (BinOp::Sub, 6, 6),
            "*" => (BinOp::Mul, 7, 7),
            "/" => (BinOp::Div, 7, 7),
            "%" => (BinOp::Mod, 7, 7),
            "^" => (BinOp::Pow, 10, 9),
            _ => return None,
        })
    }

    const UNARY: u8 = 8;

    fn subexpr(&mut self, limit: u8) -> PR<Expr> {
        let line = self.line();
        let un = if self.is("not") {
            Some(UnOp::Not)
        } else if self.is("-") {
            Some(UnOp::Neg)
        } else if self.is("#") {
            Some(UnOp::Len)
        } else {
            None
        };
        let mut e = if let Some(op) = un {
            self.at += 1;
            let x = self.subexpr(Self::UNARY)?;
            // Fold negative literals (keeps `-1` a constant).
            match (op, &x.kind) {
                (UnOp::Neg, ExprKind::Num(v)) => Expr { kind: ExprKind::Num(-v), line },
                _ => Expr { kind: ExprKind::Un(op, Box::new(x)), line },
            }
        } else {
            self.simple()?
        };
        while let Some((op, l, r)) = self.binop() {
            if l <= limit {
                break;
            }
            let line = self.line();
            self.at += 1;
            let rhs = self.subexpr(r)?;
            e = Expr { kind: ExprKind::Bin(op, Box::new(e), Box::new(rhs)), line };
        }
        Ok(e)
    }

    fn expr(&mut self) -> PR<Expr> {
        self.subexpr(0)
    }
}

/// Parses a whole script (a chunk).
pub fn parse(src: &str) -> Result<Block, ParseError> {
    let mut lx = Lexer { src: src.as_bytes(), pos: 0, line: 1 };
    let mut toks = Vec::new();
    loop {
        let t = lx.next()?;
        let eof = t.0 == Tok::Eof;
        toks.push(t);
        if eof {
            break;
        }
    }
    let mut p = Parser { toks, at: 0 };
    let b = p.block()?;
    if !matches!(p.tok(), Tok::Eof) {
        return p.err("'<eof>' expected");
    }
    Ok(b)
}
