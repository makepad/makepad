//! Expression engine: bounded lexer, recursive-descent parser, evaluator,
//! and display formatting. No Makepad types.

use crate::model::{AngleMode, BinaryOp, Constant, ErrorCode, EvalError, Function, Preview};

pub const MAX_SOURCE_BYTES: usize = 1024;
pub const MAX_TOKENS: usize = 256;
pub const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Bang,
    Percent,
    LParen,
    RParen,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Number(f64, usize),
    Const(Constant, usize),
    UnaryMinus(Box<Expr>),
    UnaryPlus(Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Call(Function, Box<Expr>),
    Fact(Box<Expr>),
    Percent(Box<Expr>),
    Group(Box<Expr>),
}

impl Expr {
    fn span_start(&self) -> usize {
        match self {
            Expr::Number(_, s) | Expr::Const(_, s) => *s,
            Expr::UnaryMinus(e)
            | Expr::UnaryPlus(e)
            | Expr::Call(_, e)
            | Expr::Fact(e)
            | Expr::Percent(e)
            | Expr::Group(e) => e.span_start(),
            Expr::Binary(_, l, _) => l.span_start(),
        }
    }
}

fn err(code: ErrorCode, byte_offset: usize) -> EvalError {
    EvalError { code, byte_offset }
}

fn finite(value: f64, at: usize) -> Result<f64, EvalError> {
    if value.is_finite() {
        Ok(value)
    } else if value.is_infinite() {
        Err(err(ErrorCode::Overflow, at))
    } else {
        Err(err(ErrorCode::Domain, at))
    }
}

/// Evaluate a canonical source string in the given angle mode.
pub fn evaluate(source: &str, angle: AngleMode) -> Result<f64, EvalError> {
    match preview(source, angle) {
        Preview::Value(v) => Ok(v),
        Preview::Incomplete => Err(err(ErrorCode::Incomplete, source.len())),
        Preview::Error(e) => Err(e),
    }
}

/// Live preview of a source string: a value, an unfinished expression, or an error.
pub fn preview(source: &str, angle: AngleMode) -> Preview {
    if source.len() > MAX_SOURCE_BYTES {
        return Preview::Error(err(ErrorCode::Limit, 0));
    }
    match parse(source) {
        Err(e) if e.code == ErrorCode::Incomplete => Preview::Incomplete,
        Err(e) => Preview::Error(e),
        Ok(ast) => match eval_expr(&ast, angle, MAX_DEPTH) {
            Ok(v) => Preview::Value(v),
            Err(e) => Preview::Error(e),
        },
    }
}

pub fn parse(source: &str) -> Result<Expr, EvalError> {
    let tokens = tokenize(source)?;
    let tokens = insert_implicit_mul(tokens)?;
    if tokens.len() > MAX_TOKENS {
        return Err(err(ErrorCode::Limit, 0));
    }
    let mut p = Parser {
        tokens: &tokens,
        i: 0,
        nodes: 0,
        depth: 1,
    };
    let ast = p.sum()?;
    if p.nodes > MAX_TOKENS {
        return Err(err(ErrorCode::Limit, p.at()));
    }
    if p.i < tokens.len() {
        return Err(err(ErrorCode::Syntax, tokens[p.i].start));
    }
    // Bound tree depth as well as recursive descent (left-associative chains
    // and postfix operators build their trees iteratively).
    let mut pending = vec![(&ast, 1)];
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            return Err(err(ErrorCode::Limit, node.span_start()));
        }
        match node {
            Expr::Number(..) | Expr::Const(..) => {}
            Expr::Binary(_, left, right) => {
                pending.push((left, depth + 1));
                pending.push((right, depth + 1));
            }
            Expr::UnaryMinus(inner) | Expr::UnaryPlus(inner) | Expr::Call(_, inner)
            | Expr::Fact(inner) | Expr::Percent(inner) | Expr::Group(inner) => {
                pending.push((inner, depth + 1));
            }
        }
    }
    Ok(ast)
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, EvalError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(err(ErrorCode::Limit, MAX_SOURCE_BYTES));
    }
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
            i += 1;
            continue;
        }
        if tokens.len() == MAX_TOKENS {
            return Err(err(ErrorCode::Limit, i));
        }
        // Unicode operators / pi, encoded as UTF-8.
        if let Some((kind, n)) = unicode_op(&source[i..]) {
            tokens.push(Token {
                kind,
                start: i,
                end: i + n,
            });
            i += n;
            continue;
        }
        match c {
            b'+' => {
                tokens.push(Token {
                    kind: TokenKind::Plus,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'-' => {
                tokens.push(Token {
                    kind: TokenKind::Minus,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'*' => {
                tokens.push(Token {
                    kind: TokenKind::Star,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'/' => {
                tokens.push(Token {
                    kind: TokenKind::Slash,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'^' => {
                tokens.push(Token {
                    kind: TokenKind::Caret,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'!' => {
                tokens.push(Token {
                    kind: TokenKind::Bang,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'%' => {
                tokens.push(Token {
                    kind: TokenKind::Percent,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'(' => {
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b')' => {
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'.' | b'0'..=b'9' => {
                let (tok, next) = lex_number(source, i)?;
                tokens.push(tok);
                i = next;
            }
            b'a'..=b'z' | b'A'..=b'Z' => {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = source[start..i].to_ascii_lowercase();
                tokens.push(Token {
                    kind: TokenKind::Ident(ident),
                    start,
                    end: i,
                });
            }
            _ => return Err(err(ErrorCode::Syntax, i)),
        }
        if tokens.len() > MAX_TOKENS {
            return Err(err(ErrorCode::Limit, i));
        }
    }
    Ok(tokens)
}

fn unicode_op(rest: &str) -> Option<(TokenKind, usize)> {
    if rest.starts_with('×') {
        Some((TokenKind::Star, '×'.len_utf8()))
    } else if rest.starts_with('÷') {
        Some((TokenKind::Slash, '÷'.len_utf8()))
    } else if rest.starts_with('−') {
        Some((TokenKind::Minus, '−'.len_utf8()))
    } else if rest.starts_with('π') {
        Some((TokenKind::Ident("pi".into()), 'π'.len_utf8()))
    } else {
        None
    }
}

fn lex_number(source: &str, start: usize) -> Result<(Token, usize), EvalError> {
    let bytes = source.as_bytes();
    let mut i = start;
    let mut saw_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        saw_digit = true;
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        let after = i + 1;
        if after < bytes.len() && bytes[after].is_ascii_digit() {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                saw_digit = true;
                i += 1;
            }
        } else if saw_digit {
            i += 1;
        } else {
            return Err(err(ErrorCode::Syntax, start));
        }
    }
    if !saw_digit {
        return Err(err(ErrorCode::Syntax, start));
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        let exp_digits = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_digits {
            i = j;
        } else if j == bytes.len() {
            return Err(err(ErrorCode::Incomplete, i));
        } else {
            return Err(err(ErrorCode::Syntax, i));
        }
    }
    let text = &source[start..i];
    let value: f64 = text.parse().map_err(|_| err(ErrorCode::Syntax, start))?;
    if !value.is_finite() {
        return Err(err(ErrorCode::Overflow, start));
    }
    Ok((
        Token {
            kind: TokenKind::Number(value),
            start,
            end: i,
        },
        i,
    ))
}

fn is_const_ident(name: &str) -> bool {
    name == "pi" || name == "e"
}

fn function_of(name: &str) -> Option<Function> {
    Some(match name {
        "inv" => Function::Reciprocal,
        "sqrt" => Function::Sqrt,
        "cbrt" => Function::Cbrt,
        "exp" => Function::Exp,
        "pow10" => Function::Pow10,
        "ln" => Function::Ln,
        "log" => Function::Log10,
        "sin" => Function::Sin,
        "cos" => Function::Cos,
        "tan" => Function::Tan,
        _ => return None,
    })
}

fn insert_implicit_mul(tokens: Vec<Token>) -> Result<Vec<Token>, EvalError> {
    if tokens.is_empty() {
        return Ok(tokens);
    }
    let mut out: Vec<Token> = Vec::with_capacity(MAX_TOKENS);
    for tok in tokens {
        if let Some(prev) = out.last() {
            if matches!((&prev.kind, &tok.kind), (TokenKind::Number(_), TokenKind::Number(_))) {
                return Err(err(ErrorCode::Syntax, tok.start));
            }
            let next_lparen = matches!(tok.kind, TokenKind::LParen);
            let next_pi = matches!(&tok.kind, TokenKind::Ident(n) if n == "pi");
            let next_fn = matches!(&tok.kind, TokenKind::Ident(n) if function_of(n.as_str()).is_some());
            let next_num = matches!(tok.kind, TokenKind::Number(_));
            let prev_rparen = matches!(prev.kind, TokenKind::RParen);
            let prev_num_or_const = matches!(&prev.kind, TokenKind::Number(_))
                || matches!(&prev.kind, TokenKind::Ident(n) if is_const_ident(n.as_str()));
            let insert = (prev_num_or_const || prev_rparen)
                && (next_lparen || next_pi || next_fn)
                || (prev_rparen && next_num);
            if insert {
                if out.len() + 2 > MAX_TOKENS {
                    return Err(err(ErrorCode::Limit, tok.start));
                }
                out.push(Token {
                    kind: TokenKind::Star,
                    start: tok.start,
                    end: tok.start,
                });
            }
        }
        if out.len() == MAX_TOKENS {
            return Err(err(ErrorCode::Limit, tok.start));
        }
        out.push(tok);
    }
    Ok(out)
}

struct Parser<'a> {
    tokens: &'a [Token],
    i: usize,
    nodes: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn descend(&mut self, parse: impl FnOnce(&mut Self) -> Result<Expr, EvalError>) -> Result<Expr, EvalError> {
        if self.depth == MAX_DEPTH {
            return Err(err(ErrorCode::Limit, self.at()));
        }
        self.depth += 1;
        let result = parse(self);
        self.depth -= 1;
        result
    }

    fn at(&self) -> usize {
        self.tokens
            .get(self.i)
            .map(|t| t.start)
            .or_else(|| self.tokens.last().map(|t| t.end))
            .unwrap_or(0)
    }

    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.i)
    }

    fn bump(&mut self) -> Option<&'a Token> {
        let t = self.tokens.get(self.i)?;
        self.i += 1;
        Some(t)
    }

    fn alloc(&mut self) -> Result<(), EvalError> {
        self.nodes += 1;
        if self.nodes > MAX_TOKENS {
            Err(err(ErrorCode::Limit, self.at()))
        } else {
            Ok(())
        }
    }

    fn expect_operand(&self) -> Result<(), EvalError> {
        if self.peek().is_none() {
            Err(err(ErrorCode::Incomplete, self.at()))
        } else {
            Ok(())
        }
    }

    fn sum(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.product()?;
        loop {
            match self.peek().map(|t| &t.kind) {
                Some(TokenKind::Plus) => {
                    self.bump();
                    self.expect_operand()?;
                    let right = self.product()?;
                    self.alloc()?;
                    left = Expr::Binary(BinaryOp::Add, Box::new(left), Box::new(right));
                }
                Some(TokenKind::Minus) => {
                    self.bump();
                    self.expect_operand()?;
                    let right = self.product()?;
                    self.alloc()?;
                    left = Expr::Binary(BinaryOp::Subtract, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn product(&mut self) -> Result<Expr, EvalError> {
        let mut left = self.unary()?;
        loop {
            match self.peek().map(|t| &t.kind) {
                Some(TokenKind::Star) => {
                    self.bump();
                    self.expect_operand()?;
                    let right = self.unary()?;
                    self.alloc()?;
                    left = Expr::Binary(BinaryOp::Multiply, Box::new(left), Box::new(right));
                }
                Some(TokenKind::Slash) => {
                    self.bump();
                    self.expect_operand()?;
                    let right = self.unary()?;
                    self.alloc()?;
                    left = Expr::Binary(BinaryOp::Divide, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, EvalError> {
        match self.peek().map(|t| &t.kind) {
            Some(TokenKind::Plus) => {
                self.bump();
                self.expect_operand()?;
                let inner = self.descend(Self::unary)?;
                self.alloc()?;
                Ok(Expr::UnaryPlus(Box::new(inner)))
            }
            Some(TokenKind::Minus) => {
                self.bump();
                self.expect_operand()?;
                let inner = self.descend(Self::unary)?;
                self.alloc()?;
                Ok(Expr::UnaryMinus(Box::new(inner)))
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<Expr, EvalError> {
        let left = self.postfix()?;
        if matches!(self.peek().map(|t| &t.kind), Some(TokenKind::Caret)) {
            self.bump();
            self.expect_operand()?;
            let right = self.descend(Self::unary)?;
            self.alloc()?;
            Ok(Expr::Binary(BinaryOp::Power, Box::new(left), Box::new(right)))
        } else {
            Ok(left)
        }
    }

    fn postfix(&mut self) -> Result<Expr, EvalError> {
        let mut inner = self.atom()?;
        loop {
            match self.peek().map(|t| &t.kind) {
                Some(TokenKind::Bang) => {
                    self.bump();
                    self.alloc()?;
                    inner = Expr::Fact(Box::new(inner));
                }
                Some(TokenKind::Percent) => {
                    self.bump();
                    self.alloc()?;
                    inner = Expr::Percent(Box::new(inner));
                }
                _ => break,
            }
        }
        Ok(inner)
    }

    fn atom(&mut self) -> Result<Expr, EvalError> {
        let Some(tok) = self.bump() else {
            return Err(err(ErrorCode::Incomplete, self.at()));
        };
        match &tok.kind {
            TokenKind::Number(n) => {
                self.alloc()?;
                Ok(Expr::Number(*n, tok.start))
            }
            TokenKind::Ident(name) if name == "pi" => {
                self.alloc()?;
                Ok(Expr::Const(Constant::Pi, tok.start))
            }
            TokenKind::Ident(name) if name == "e" => {
                self.alloc()?;
                Ok(Expr::Const(Constant::E, tok.start))
            }
            TokenKind::Ident(name) => {
                let Some(func) = function_of(name) else {
                    return Err(err(ErrorCode::UnknownName, tok.start));
                };
                match self.peek().map(|t| &t.kind) {
                    Some(TokenKind::LParen) => {
                        self.bump();
                        self.expect_operand()?;
                        let arg = self.descend(Self::sum)?;
                        match self.peek().map(|t| &t.kind) {
                            Some(TokenKind::RParen) => {
                                self.bump();
                            }
                            None => return Err(err(ErrorCode::Incomplete, self.at())),
                            Some(_) => return Err(err(ErrorCode::Syntax, self.at())),
                        }
                        self.alloc()?;
                        Ok(Expr::Call(func, Box::new(arg)))
                    }
                    None => Err(err(ErrorCode::Incomplete, tok.end)),
                    Some(_) => Err(err(ErrorCode::Syntax, tok.end)),
                }
            }
            TokenKind::LParen => {
                self.expect_operand()?;
                let inner = self.descend(Self::sum)?;
                match self.peek().map(|t| &t.kind) {
                    Some(TokenKind::RParen) => {
                        self.bump();
                    }
                    None => return Err(err(ErrorCode::Incomplete, self.at())),
                    Some(_) => return Err(err(ErrorCode::Syntax, self.at())),
                }
                self.alloc()?;
                Ok(Expr::Group(Box::new(inner)))
            }
            _ => Err(err(ErrorCode::Syntax, tok.start)),
        }
    }
}

fn strip_groups(expr: &Expr) -> &Expr {
    let mut e = expr;
    while let Expr::Group(inner) = e {
        e = inner;
    }
    e
}

fn is_bare_percent(expr: &Expr) -> bool {
    matches!(strip_groups(expr), Expr::Percent(_))
}

fn percent_inner(expr: &Expr) -> &Expr {
    match strip_groups(expr) {
        Expr::Percent(inner) => inner,
        other => other,
    }
}

fn eval_expr(expr: &Expr, angle: AngleMode, depth: usize) -> Result<f64, EvalError> {
    if depth == 0 {
        return Err(err(ErrorCode::Limit, expr.span_start()));
    }
    let at = expr.span_start();
    match expr {
        Expr::Number(n, at) => finite(*n, *at),
        Expr::Const(Constant::Pi, at) => finite(std::f64::consts::PI, *at),
        Expr::Const(Constant::E, at) => finite(std::f64::consts::E, *at),
        Expr::UnaryPlus(e) => eval_expr(e, angle, depth - 1),
        Expr::UnaryMinus(e) => {
            let v = eval_expr(e, angle, depth - 1)?;
            finite(-v, at)
        }
        Expr::Group(e) => eval_expr(e, angle, depth - 1),
        Expr::Percent(e) => {
            let v = eval_expr(e, angle, depth - 1)?;
            finite(v / 100.0, at)
        }
        Expr::Fact(e) => {
            let v = eval_expr(e, angle, depth - 1)?;
            factorial(v, at)
        }
        Expr::Call(func, arg) => {
            let v = eval_expr(arg, angle, depth - 1)?;
            eval_func(*func, v, angle, at)
        }
        Expr::Binary(op, left, right) => match op {
            BinaryOp::Add | BinaryOp::Subtract if is_bare_percent(right) => {
                let a = eval_expr(left, angle, depth - 1)?;
                let b = eval_expr(percent_inner(right), angle, depth - 1)?;
                let rel = a * b / 100.0;
                let out = if *op == BinaryOp::Add { a + rel } else { a - rel };
                finite(out, at)
            }
            BinaryOp::Add => {
                let a = eval_expr(left, angle, depth - 1)?;
                let b = eval_expr(right, angle, depth - 1)?;
                finite(a + b, at)
            }
            BinaryOp::Subtract => {
                let a = eval_expr(left, angle, depth - 1)?;
                let b = eval_expr(right, angle, depth - 1)?;
                finite(a - b, at)
            }
            BinaryOp::Multiply => {
                let a = eval_expr(left, angle, depth - 1)?;
                let b = eval_expr(right, angle, depth - 1)?;
                finite(a * b, at)
            }
            BinaryOp::Divide => {
                let a = eval_expr(left, angle, depth - 1)?;
                let b = eval_expr(right, angle, depth - 1)?;
                if b == 0.0 {
                    Err(err(ErrorCode::DivisionByZero, at))
                } else {
                    finite(a / b, at)
                }
            }
            BinaryOp::Power => {
                let a = eval_expr(left, angle, depth - 1)?;
                let b = eval_expr(right, angle, depth - 1)?;
                pow(a, b, at)
            }
        },
    }
}

fn eval_func(func: Function, v: f64, angle: AngleMode, at: usize) -> Result<f64, EvalError> {
    match func {
        Function::Reciprocal => {
            if v == 0.0 {
                Err(err(ErrorCode::DivisionByZero, at))
            } else {
                finite(1.0 / v, at)
            }
        }
        Function::Sqrt => {
            if v < 0.0 {
                Err(err(ErrorCode::Domain, at))
            } else {
                finite(v.sqrt(), at)
            }
        }
        Function::Cbrt => finite(v.cbrt(), at),
        Function::Exp => finite(v.exp(), at),
        Function::Pow10 => finite(10.0_f64.powf(v), at),
        Function::Ln => {
            if v <= 0.0 {
                Err(err(ErrorCode::Domain, at))
            } else {
                finite(v.ln(), at)
            }
        }
        Function::Log10 => {
            if v <= 0.0 {
                Err(err(ErrorCode::Domain, at))
            } else {
                finite(v.log10(), at)
            }
        }
        Function::Sin => finite(trig_sin(v, angle), at),
        Function::Cos => finite(trig_cos(v, angle), at),
        Function::Tan => trig_tan(v, angle, at),
    }
}

fn reduce_degrees(deg: f64) -> f64 {
    let mut r = deg % 360.0;
    if r < 0.0 {
        r += 360.0;
    }
    r
}

fn exact_quadrant(deg: f64) -> Option<i32> {
    let r = reduce_degrees(deg);
    if r == 0.0 || r == 360.0 {
        Some(0)
    } else if r == 90.0 {
        Some(90)
    } else if r == 180.0 {
        Some(180)
    } else if r == 270.0 {
        Some(270)
    } else {
        None
    }
}

fn trig_sin(v: f64, angle: AngleMode) -> f64 {
    match angle {
        AngleMode::Degrees => match exact_quadrant(v) {
            Some(0) => 0.0,
            Some(90) => 1.0,
            Some(180) => 0.0,
            Some(270) => -1.0,
            _ => reduce_degrees(v).to_radians().sin(),
        },
        AngleMode::Radians => v.sin(),
    }
}

fn trig_cos(v: f64, angle: AngleMode) -> f64 {
    match angle {
        AngleMode::Degrees => match exact_quadrant(v) {
            Some(0) => 1.0,
            Some(90) => 0.0,
            Some(180) => -1.0,
            Some(270) => 0.0,
            _ => reduce_degrees(v).to_radians().cos(),
        },
        AngleMode::Radians => v.cos(),
    }
}

fn trig_tan(v: f64, angle: AngleMode, at: usize) -> Result<f64, EvalError> {
    match angle {
        AngleMode::Degrees => match exact_quadrant(v) {
            Some(0) | Some(180) => Ok(0.0),
            Some(90) | Some(270) => Err(err(ErrorCode::Domain, at)),
            _ => {
                let rad = reduce_degrees(v).to_radians();
                if rad.cos().abs() < 1e-15 {
                    Err(err(ErrorCode::Domain, at))
                } else {
                    finite(rad.tan(), at)
                }
            }
        },
        AngleMode::Radians => {
            if v.cos().abs() < 1e-15 {
                Err(err(ErrorCode::Domain, at))
            } else {
                finite(v.tan(), at)
            }
        }
    }
}

fn pow(base: f64, exp: f64, at: usize) -> Result<f64, EvalError> {
    if base == 0.0 && exp == 0.0 {
        return Ok(1.0);
    }
    if base == 0.0 && exp < 0.0 {
        return Err(err(ErrorCode::DivisionByZero, at));
    }
    if base < 0.0 {
        let rounded = exp.round();
        if (exp - rounded).abs() > 1e-12 {
            return Err(err(ErrorCode::Domain, at));
        }
    }
    finite(base.powf(exp), at)
}

fn factorial(v: f64, at: usize) -> Result<f64, EvalError> {
    if v < 0.0 || v.fract() != 0.0 {
        return Err(err(ErrorCode::Domain, at));
    }
    if v > 170.0 {
        return Err(err(ErrorCode::Overflow, at));
    }
    let mut acc = 1.0_f64;
    for k in 2..=v as u32 {
        acc *= k as f64;
        if !acc.is_finite() {
            return Err(err(ErrorCode::Overflow, at));
        }
    }
    Ok(acc)
}

/// Round-trip a stored f64 into a canonical numeric literal (no grouping).
pub fn roundtrip_literal(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    let grouped = format_number(value);
    let mut plain = grouped.replace(',', "");
    if let Ok(back) = plain.parse::<f64>() {
        if back == value {
            return plain;
        }
    }
    plain = format!("{value:?}");
    plain
}

/// Apple-like display formatting: 16 significant digits, grouping, scientific
/// outside exponent −6…15. Negative zero becomes `0`.
pub fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return "Error".to_string();
    }
    let neg = value.is_sign_negative() && value != 0.0;
    let abs = value.abs();
    if abs == 0.0 {
        return "0".to_string();
    }
    let (digits, exp) = round_sig16(abs);
    let body = if exp >= -6 && exp <= 15 {
        format_fixed(&digits, exp)
    } else {
        format_scientific(&digits, exp)
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

fn round_sig16(abs: f64) -> (Vec<u8>, i32) {
    let s = format!("{:.15e}", abs);
    let (mant, exp_s) = s.split_once(['e', 'E']).unwrap_or((&s, "0"));
    let exp: i32 = exp_s.parse().unwrap_or(0);
    let mut digits = Vec::with_capacity(16);
    for c in mant.chars() {
        if c.is_ascii_digit() {
            digits.push(c as u8 - b'0');
        }
    }
    if digits.is_empty() {
        digits.push(0);
    }
    while digits.len() < 16 {
        digits.push(0);
    }
    digits.truncate(16);
    (digits, exp)
}

fn format_fixed(digits: &[u8], exp: i32) -> String {
    if exp >= 0 {
        let int_len = exp as usize + 1;
        let mut int_digits = String::new();
        for i in 0..int_len {
            let d = if i < digits.len() { digits[i] } else { 0 };
            int_digits.push((b'0' + d) as char);
        }
        let grouped = group_commas(&int_digits);
        if int_len >= digits.len() {
            grouped
        } else {
            let mut frac = String::new();
            for &d in &digits[int_len..] {
                frac.push((b'0' + d) as char);
            }
            let frac = frac.trim_end_matches('0');
            if frac.is_empty() {
                grouped
            } else {
                format!("{grouped}.{frac}")
            }
        }
    } else {
        let zeros = (-exp - 1) as usize;
        let mut frac = "0".repeat(zeros);
        for &d in digits {
            frac.push((b'0' + d) as char);
        }
        let frac = frac.trim_end_matches('0');
        format!("0.{frac}")
    }
}

fn format_scientific(digits: &[u8], exp: i32) -> String {
    let mut frac = String::new();
    for &d in digits.iter().skip(1) {
        frac.push((b'0' + d) as char);
    }
    let frac = frac.trim_end_matches('0');
    let lead = (b'0' + digits[0]) as char;
    if frac.is_empty() {
        format!("{lead}e{exp:+}")
    } else {
        format!("{lead}.{frac}e{exp:+}")
    }
}

fn group_commas(int_digits: &str) -> String {
    let mut out = String::new();
    let bytes = int_digits.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// Glyph form of a canonical source string for the expression line.
pub fn display_expression(source: &str) -> String {
    match tokenize(source) {
        Ok(tokens) if !tokens.is_empty() => {
            let mut out = String::new();
            let mut last_end = 0;
            for tok in &tokens {
                if tok.start > last_end {
                    out.push_str(&source[last_end..tok.start]);
                }
                match &tok.kind {
                    TokenKind::Star => out.push('×'),
                    TokenKind::Slash => out.push('÷'),
                    TokenKind::Minus => out.push('−'),
                    TokenKind::Ident(n) if n == "pi" => out.push('π'),
                    _ => out.push_str(&source[tok.start..tok.end]),
                }
                last_end = tok.end;
            }
            if last_end < source.len() {
                out.push_str(&source[last_end..]);
            }
            out
        }
        _ => source
            .replace('*', "×")
            .replace('/', "÷")
            .replace('-', "−")
            .replace("pi", "π"),
    }
}

pub fn error_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Incomplete => "Incomplete expression",
        ErrorCode::Syntax => "Invalid expression",
        ErrorCode::UnknownName => "Unknown name",
        ErrorCode::DivisionByZero => "Cannot divide by zero",
        ErrorCode::Domain => "Not a number",
        ErrorCode::Overflow => "Overflow",
        ErrorCode::Limit => "Expression too long",
    }
}

/// Close unmatched opening parentheses when the tail is a complete operand.
pub fn close_unmatched_parens(source: &str) -> Option<String> {
    let Ok(tokens) = tokenize(source) else {
        return None;
    };
    if tokens.is_empty() {
        return None;
    }
    if !tail_is_complete_operand(&tokens) {
        return None;
    }
    let mut depth = 0i32;
    for tok in &tokens {
        match tok.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    if depth == 0 {
        return Some(source.to_string());
    }
    let mut out = source.to_string();
    for _ in 0..depth {
        out.push(')');
    }
    Some(out)
}

fn tail_is_complete_operand(tokens: &[Token]) -> bool {
    match tokens.last().map(|t| &t.kind) {
        Some(
            TokenKind::Number(_)
            | TokenKind::Ident(_)
            | TokenKind::RParen
            | TokenKind::Bang
            | TokenKind::Percent,
        ) => true,
        _ => false,
    }
}

/// The last operand's byte span, if the source ends on a completed/partial operand.
pub fn trailing_operand_span(source: &str) -> Option<(usize, usize)> {
    let tokens = tokenize(source).ok()?;
    trailing_operand_from_tokens(&tokens)
}

pub fn trailing_operand_from_tokens(tokens: &[Token]) -> Option<(usize, usize)> {
    let mut i = tokens.len().checked_sub(1)?;
    let end = tokens[i].end;
    while matches!(tokens[i].kind, TokenKind::Bang | TokenKind::Percent) {
        i = i.checked_sub(1)?;
    }
    match tokens[i].kind {
        TokenKind::Number(_) | TokenKind::Ident(_) => {}
        TokenKind::RParen => {
            let mut depth = 1;
            while depth > 0 {
                i = i.checked_sub(1)?;
                match tokens[i].kind {
                    TokenKind::RParen => depth += 1,
                    TokenKind::LParen => depth -= 1,
                    _ => {}
                }
            }
            if i > 0 && matches!(&tokens[i - 1].kind, TokenKind::Ident(n) if function_of(n).is_some()) {
                i -= 1;
            }
        }
        _ => return None,
    }
    // A sign belongs to this operand only at an operand start; the minus in
    // `2-3` remains the binary operator, while `2*-3` includes the second minus.
    while i > 0 && matches!(tokens[i - 1].kind, TokenKind::Plus | TokenKind::Minus) {
        let unary = i == 1 || matches!(tokens[i - 2].kind,
            TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::Slash
            | TokenKind::Caret | TokenKind::LParen);
        if !unary { break; }
        i -= 1;
    }
    Some((tokens[i].start, end))
}

/// True when `source` ends with a generated function opener such as `sin(`.
pub fn trailing_function_opener(source: &str) -> Option<(usize, usize)> {
    const NAMES: &[&str] = &[
        "inv(", "sqrt(", "cbrt(", "exp(", "pow10(", "ln(", "log(", "sin(", "cos(", "tan(",
    ];
    for name in NAMES {
        if source.ends_with(name) {
            let start = source.len() - name.len();
            return Some((start, source.len()));
        }
    }
    None
}

pub fn last_token_is_number(source: &str) -> bool {
    tokenize(source)
        .ok()
        .and_then(|t| t.last().map(|tok| matches!(tok.kind, TokenKind::Number(_))))
        .unwrap_or(false)
}

pub fn last_number_spelling<'a>(source: &'a str) -> Option<&'a str> {
    let tokens = tokenize(source).ok()?;
    let last = tokens.last()?;
    if matches!(last.kind, TokenKind::Number(_)) {
        Some(&source[last.start..last.end])
    } else {
        None
    }
}

/// The final root binary operation and its evaluated right-hand side, used for
/// repeated equals. Relative percent is reported separately.
pub fn root_repeat(source: &str, angle: AngleMode) -> Option<(BinaryOp, f64, bool)> {
    let ast = parse(source).ok()?;
    match &ast {
        Expr::Binary(op, _left, right) => {
            let relative = matches!(*op, BinaryOp::Add | BinaryOp::Subtract) && is_bare_percent(right);
            let rhs = if relative {
                eval_expr(percent_inner(right), angle, MAX_DEPTH).ok()? / 100.0
            } else {
                eval_expr(right, angle, MAX_DEPTH).ok()?
            };
            Some((*op, rhs, relative))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> Result<f64, ErrorCode> {
        evaluate(s, AngleMode::Degrees).map_err(|e| e.code)
    }

    fn ev_rad(s: &str) -> Result<f64, ErrorCode> {
        evaluate(s, AngleMode::Radians).map_err(|e| e.code)
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() <= 1e-12 * (1.0 + b.abs()) || a == b, "{a} != {b}");
    }

    #[test]
    fn parsing_whitespace_unicode_literals_and_functions() {
        close(ev("  2 + 3  ").unwrap(), 5.0);
        close(ev("6×7").unwrap(), 42.0);
        close(ev("8÷2").unwrap(), 4.0);
        close(ev("5−1").unwrap(), 4.0);
        close(ev(".5").unwrap(), 0.5);
        close(ev("1.").unwrap(), 1.0);
        close(ev("1.2e-3").unwrap(), 0.0012);
        close(ev("1.2E+3").unwrap(), 1200.0);
        close(ev("sqrt(9)").unwrap(), 3.0);
        close(ev("2pi").unwrap(), 2.0 * std::f64::consts::PI);
        close(ev("2(3+4)").unwrap(), 14.0);
        close(ev("(2)(3)").unwrap(), 6.0);
        close(ev("(2)3").unwrap(), 6.0);
        close(ev("2sqrt(4)").unwrap(), 4.0);
        assert_eq!(ev("2 3").unwrap_err(), ErrorCode::Syntax);
        assert_eq!(ev("foo(1)").unwrap_err(), ErrorCode::UnknownName);
        assert_eq!(ev("2+3 4").unwrap_err(), ErrorCode::Syntax);
        assert_eq!(ev("1.2e").unwrap_err(), ErrorCode::Incomplete);
        assert_eq!(ev("1.2e+").unwrap_err(), ErrorCode::Incomplete);
        assert_eq!(ev("1.2e+x").unwrap_err(), ErrorCode::Syntax);
        close(ev("2*e").unwrap(), 2.0 * std::f64::consts::E);
        assert!(matches!(preview("2+", AngleMode::Degrees), Preview::Incomplete));
        assert!(matches!(preview("sin(", AngleMode::Degrees), Preview::Incomplete));
    }

    #[test]
    fn precedence_matches_the_spec() {
        close(ev("2+3*4").unwrap(), 14.0);
        close(ev("(2+3)*4").unwrap(), 20.0);
        close(ev("2^3^2").unwrap(), 512.0);
        close(ev("-2^2").unwrap(), -4.0);
        close(ev("(-2)^2").unwrap(), 4.0);
        close(ev("2^-3").unwrap(), 0.125);
    }

    #[test]
    fn scientific_functions_degrees_and_radians() {
        close(ev("2^2").unwrap(), 4.0);
        close(ev("2^3").unwrap(), 8.0);
        close(ev("inv(4)").unwrap(), 0.25);
        close(ev("sqrt(16)").unwrap(), 4.0);
        close(ev("cbrt(-8)").unwrap(), -2.0);
        close(ev("exp(0)").unwrap(), 1.0);
        close(ev("pow10(2)").unwrap(), 100.0);
        close(ev("ln(e)").unwrap(), 1.0);
        close(ev("log(100)").unwrap(), 2.0);
        close(ev("sin(90)").unwrap(), 1.0);
        close(ev("cos(180)").unwrap(), -1.0);
        close(ev("tan(45)").unwrap(), 1.0);
        close(ev("sin(0)").unwrap(), 0.0);
        close(ev("cos(90)").unwrap(), 0.0);
        close(ev("tan(0)").unwrap(), 0.0);
        close(ev("1!").unwrap(), 1.0);
        close(ev("0!").unwrap(), 1.0);
        close(ev("pi").unwrap(), std::f64::consts::PI);
        close(ev("e").unwrap(), std::f64::consts::E);
        close(ev_rad("sin(0)").unwrap(), 0.0);
        close(ev_rad("cos(0)").unwrap(), 1.0);
        let half_pi = std::f64::consts::FRAC_PI_2;
        close(ev_rad("sin(pi/2)").unwrap(), 1.0);
        assert_eq!(ev_rad(&format!("tan({half_pi})")).unwrap_err(), ErrorCode::Domain);
        assert_eq!(ev("tan(90)").unwrap_err(), ErrorCode::Domain);
        assert_eq!(ev("tan(270)").unwrap_err(), ErrorCode::Domain);
    }

    #[test]
    fn percent_is_contextual() {
        close(ev("200+10%").unwrap(), 220.0);
        close(ev("200-10%").unwrap(), 180.0);
        close(ev("200*10%").unwrap(), 20.0);
        close(ev("200/10%").unwrap(), 2000.0);
        close(ev("(200+10)%").unwrap(), 2.1);
        close(ev("200+10%*2").unwrap(), 200.2);
        close(ev("200+(10%)").unwrap(), 220.0);
        close(ev("10%").unwrap(), 0.1);
    }

    #[test]
    fn domains_overflow_and_special_powers() {
        assert_eq!(ev("1/0").unwrap_err(), ErrorCode::DivisionByZero);
        assert_eq!(ev("inv(0)").unwrap_err(), ErrorCode::DivisionByZero);
        assert_eq!(ev("sqrt(-1)").unwrap_err(), ErrorCode::Domain);
        close(ev("cbrt(-8)").unwrap(), -2.0);
        assert_eq!(ev("ln(0)").unwrap_err(), ErrorCode::Domain);
        assert_eq!(ev("log(-1)").unwrap_err(), ErrorCode::Domain);
        close(ev("0!").unwrap(), 1.0);
        assert!(ev("170!").unwrap().is_finite());
        assert_eq!(ev("171!").unwrap_err(), ErrorCode::Overflow);
        assert_eq!(ev("(-1)!").unwrap_err(), ErrorCode::Domain);
        assert_eq!(ev("2.5!").unwrap_err(), ErrorCode::Domain);
        close(ev("0^0").unwrap(), 1.0);
        assert_eq!(ev("0^-1").unwrap_err(), ErrorCode::DivisionByZero);
        assert_eq!(ev("(-2)^0.5").unwrap_err(), ErrorCode::Domain);
        assert_eq!(ev("1e1000").unwrap_err(), ErrorCode::Overflow);
        close(ev("1e-400").unwrap(), 0.0);
    }

    #[test]
    fn formatting_rounding_grouping_and_exponents() {
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(1.0 / 3.0), "0.3333333333333333");
        assert_eq!(format_number(1_234_567.89), "1,234,567.89");
        assert_eq!(format_number(1e16), "1e+16");
        assert_eq!(format_number(1e-7), "1e-7");
        assert_eq!(format_number(1e-6), "0.000001");
        assert_eq!(format_number(1e15), "1,000,000,000,000,000");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(-12.5), "-12.5");
        assert_eq!(format_number(1.0), "1");
        let continued = roundtrip_literal(1.0 / 3.0);
        assert_eq!(continued.parse::<f64>().unwrap(), 1.0 / 3.0);
        // Carry across the exponent boundary: 9.999…e15 rounds into 1e16.
        let almost = 9.999999999999999e15;
        let shown = format_number(almost);
        assert!(shown == "10,000,000,000,000,000" || shown.starts_with("1e+"), "{shown}");
    }

    #[test]
    fn bounds_do_not_panic() {
        let too_long = "1+".repeat(600);
        assert_eq!(ev(&too_long).unwrap_err(), ErrorCode::Limit);
        let nested = format!("{}1{}", "(".repeat(40), ")".repeat(40));
        let r = ev(&nested);
        assert!(r.is_err());
        let many = "1+".repeat(200) + "1";
        let _ = ev(&many);
    }

    #[test]
    fn display_glyphs_and_paren_repair() {
        assert_eq!(display_expression("2*3/4-pi"), "2×3÷4−π");
        assert_eq!(close_unmatched_parens("2+(3"), Some("2+(3)".into()));
        assert_eq!(close_unmatched_parens("2+"), None);
        assert_eq!(close_unmatched_parens("2+(3*4)"), Some("2+(3*4)".into()));
    }

    #[test]
    fn trailing_operand_and_function_opener() {
        assert_eq!(trailing_operand_span("2+35"), Some((2, 4)));
        assert_eq!(trailing_operand_span("2+"), None);
        assert_eq!(trailing_operand_span("-9"), Some((0, 2)));
        assert_eq!(trailing_operand_span("2*-3"), Some((2, 4)));
        assert_eq!(trailing_operand_span("2-3"), Some((2, 3)));
        assert_eq!(trailing_operand_span("sin(30)"), Some((0, 7)));
        assert_eq!(trailing_function_opener("2+sin("), Some((2, 6)));
        assert_eq!(trailing_function_opener("2+3"), None);
    }
    #[test]
    fn fractional_factorials_never_round_into_integers() {
        for source in ["1.0000000000005!", "0.0000000000005!", "(-0.0000000000005)!"] {
            assert_eq!(ev(source), Err(ErrorCode::Domain), "{source}");
        }
        assert_eq!(ev("1!"), Ok(1.0));
        assert!(ev("170!").unwrap().is_finite());
        assert_eq!(ev("171!"), Err(ErrorCode::Overflow));
    }

    #[test]
    fn lexer_parser_enforce_exact_byte_token_and_depth_bounds() {
        let exact_bytes = format!("{}1", " ".repeat(MAX_SOURCE_BYTES - 1));
        assert_eq!(ev(&exact_bytes), Ok(1.0));
        assert!(parse(&exact_bytes).is_ok());
        for result in [tokenize(&(exact_bytes.clone() + " ")).map(|_| ()), parse(&(exact_bytes + " ")).map(|_| ())] {
            assert_eq!(result.unwrap_err().code, ErrorCode::Limit);
        }
        assert_eq!(tokenize(&"+".repeat(MAX_TOKENS)).unwrap().len(), MAX_TOKENS);
        assert_eq!(tokenize(&"+".repeat(MAX_TOKENS + 1)).unwrap_err().code, ErrorCode::Limit);
        assert_eq!(tokenize(&"−".repeat(MAX_TOKENS)).unwrap().len(), MAX_TOKENS);
        assert_eq!(tokenize(&"−".repeat(MAX_TOKENS + 1)).unwrap_err().code, ErrorCode::Limit);
        let implicit = format!("{}!", "pi ".repeat(128));
        assert_eq!(insert_implicit_mul(tokenize(&implicit).unwrap()).unwrap().len(), MAX_TOKENS);
        assert_eq!(insert_implicit_mul(tokenize(&(implicit + "!")).unwrap()).unwrap_err().code, ErrorCode::Limit);
        let nested = format!("{}1{}", "(".repeat(MAX_DEPTH - 1), ")".repeat(MAX_DEPTH - 1));
        assert_eq!(ev(&nested), Ok(1.0));
        assert_eq!(parse(&format!("({nested})")).unwrap_err().code, ErrorCode::Limit);
        assert_eq!(parse(&"(".repeat(100)).unwrap_err().code, ErrorCode::Limit);
        assert_eq!(parse(&("-".repeat(100) + "1")).unwrap_err().code, ErrorCode::Limit);
        assert_eq!(parse(&("1^".repeat(100) + "1")).unwrap_err().code, ErrorCode::Limit);
        assert_eq!(parse(&("1".to_owned() + &"!".repeat(MAX_DEPTH))).unwrap_err().code, ErrorCode::Limit);
    }

}
