//! The formula engine: a pure, self-contained parser + evaluator for
//! spreadsheet expressions. No Makepad types appear here on purpose — the
//! whole module is exercised by `cargo test -p mpsheets` without a UI.
//!
//! Grammar (loosest to tightest binding, Excel's order):
//!
//! ```text
//! compare := concat (('=' | '<>' | '<' | '<=' | '>' | '>=') concat)*
//! concat  := add ('&' add)*
//! add     := mul (('+' | '-') mul)*
//! mul     := pow (('*' | '/') pow)*
//! pow     := unary ('^' pow)?            // right associative
//! unary   := ('-' | '+') unary | atom
//! atom    := number | string | TRUE | FALSE | '(' compare ')'
//!          | name '(' args ')' | ref (':' ref)?
//! ```
//!
//! Note that unary minus binds *tighter* than `^`, so `-3^2` is `9` — the
//! behaviour Excel has and most other languages do not.

use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// values
// ---------------------------------------------------------------------------

/// The error values a cell can hold. These are real values, not exceptions:
/// they propagate through arithmetic exactly like numbers do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrKind {
    /// Division by zero.
    Div0,
    /// A reference that does not exist (e.g. filled off the top of the sheet).
    Ref,
    /// An unknown function or name.
    Name,
    /// A value of the wrong type for the operation.
    Value,
    /// This cell takes part in a reference cycle.
    Circ,
    /// A numeric result that is not finite (overflow, sqrt of a negative).
    Num,
    /// The formula text does not parse.
    Parse,
}

impl ErrKind {
    pub fn text(self) -> &'static str {
        match self {
            ErrKind::Div0 => "#DIV/0!",
            ErrKind::Ref => "#REF!",
            ErrKind::Name => "#NAME?",
            ErrKind::Value => "#VALUE!",
            ErrKind::Circ => "#CIRC!",
            ErrKind::Num => "#NUM!",
            ErrKind::Parse => "#ERROR!",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Num(f64),
    Text(String),
    Bool(bool),
    Err(ErrKind),
}

impl Value {
    /// General-format rendering — what `&` concatenation and the formula bar
    /// see. Cell *display* additionally applies the cell's number format.
    pub fn to_text(&self) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Num(n) => format_general(*n),
            Value::Text(s) => s.clone(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            Value::Err(e) => e.text().to_string(),
        }
    }

    pub fn as_num(&self) -> Result<f64, ErrKind> {
        match self {
            Value::Empty => Ok(0.0),
            Value::Num(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Text(s) => {
                let t = s.trim();
                if t.is_empty() {
                    return Ok(0.0);
                }
                t.parse::<f64>().map_err(|_| ErrKind::Value)
            }
            Value::Err(e) => Err(*e),
        }
    }

    pub fn as_bool(&self) -> Result<bool, ErrKind> {
        match self {
            Value::Empty => Ok(false),
            Value::Num(n) => Ok(*n != 0.0),
            Value::Bool(b) => Ok(*b),
            Value::Text(s) => match s.trim().to_ascii_uppercase().as_str() {
                "TRUE" => Ok(true),
                "FALSE" | "" => Ok(false),
                _ => Err(ErrKind::Value),
            },
            Value::Err(e) => Err(*e),
        }
    }

    pub fn err(&self) -> Option<ErrKind> {
        match self {
            Value::Err(e) => Some(*e),
            _ => None,
        }
    }

    pub fn is_num(&self) -> bool {
        matches!(self, Value::Num(_))
    }
}

/// General number formatting: integers plain, fractions rounded to ~10
/// significant decimals so `0.1 + 0.2` reads as `0.3` rather than
/// `0.30000000000000004`.
pub fn format_general(n: f64) -> String {
    if !n.is_finite() {
        return ErrKind::Num.text().to_string();
    }
    if n == 0.0 {
        return "0".to_string();
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let mag = n.abs().log10().floor() as i32;
    let dec = (9 - mag).clamp(0, 15) as usize;
    let s = format!("{:.*}", dec, n);
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    // A tiny magnitude rounded away entirely: fall back to scientific.
    if s == "0" || s == "-0" {
        return format!("{:e}", n);
    }
    s
}

// ---------------------------------------------------------------------------
// references
// ---------------------------------------------------------------------------

/// An A1-style reference. `abs_row`/`abs_col` record the `$` markers, which is
/// what makes fill-handle translation correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellRef {
    pub row: usize,
    pub col: usize,
    pub abs_row: bool,
    pub abs_col: bool,
}

impl CellRef {
    pub fn to_a1(self) -> String {
        format!(
            "{}{}{}{}",
            if self.abs_col { "$" } else { "" },
            col_letters(self.col),
            if self.abs_row { "$" } else { "" },
            self.row + 1
        )
    }

    /// Shift by a relative offset, honouring the `$` anchors. `None` when the
    /// result would fall off the top/left of the sheet — that is a `#REF!`.
    pub fn translate(self, drow: isize, dcol: isize) -> Option<Self> {
        let row = if self.abs_row {
            self.row as isize
        } else {
            self.row as isize + drow
        };
        let col = if self.abs_col {
            self.col as isize
        } else {
            self.col as isize + dcol
        };
        if row < 0 || col < 0 {
            return None;
        }
        Some(Self {
            row: row as usize,
            col: col as usize,
            ..self
        })
    }
}

/// Column index to spreadsheet letters: 0 -> "A", 26 -> "AA".
pub fn col_letters(col: usize) -> String {
    let mut n = col;
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// Parse spreadsheet letters into a column index: "A" -> 0, "AA" -> 26.
pub fn letters_col(letters: &str) -> Option<usize> {
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut col = 0usize;
    for b in letters.bytes() {
        if !b.is_ascii_alphabetic() {
            return None;
        }
        col = col * 26 + (b.to_ascii_uppercase() - b'A') as usize + 1;
    }
    Some(col - 1)
}

/// "B12" / "$B$12" -> CellRef.
pub fn parse_a1(text: &str) -> Option<CellRef> {
    let mut chars = text.chars().peekable();
    let abs_col = chars.peek() == Some(&'$');
    if abs_col {
        chars.next();
    }
    let mut letters = String::new();
    while let Some(c) = chars.peek() {
        if c.is_ascii_alphabetic() {
            letters.push(*c);
            chars.next();
        } else {
            break;
        }
    }
    let abs_row = chars.peek() == Some(&'$');
    if abs_row {
        chars.next();
    }
    let digits: String = chars.collect();
    if letters.is_empty() || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let col = letters_col(&letters)?;
    let row: usize = digits.parse().ok()?;
    if row == 0 {
        return None;
    }
    Some(CellRef {
        row: row - 1,
        col,
        abs_row,
        abs_col,
    })
}

/// "B12" for (row 11, col 1) — the plain, unanchored name.
pub fn ref_name(row: usize, col: usize) -> String {
    format!("{}{}", col_letters(col), row + 1)
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    fn prec(self) -> u8 {
        match self {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 1,
            BinOp::Concat => 2,
            BinOp::Add | BinOp::Sub => 3,
            BinOp::Mul | BinOp::Div => 4,
            BinOp::Pow => 5,
        }
    }

    fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Concat => "&",
            BinOp::Eq => "=",
            BinOp::Ne => "<>",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    /// A literal error, e.g. what a `#REF!`-producing translation leaves behind.
    ErrLit(ErrKind),
    Ref(CellRef),
    Range(CellRef, CellRef),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

impl Expr {
    fn prec(&self) -> u8 {
        match self {
            Expr::Bin(op, ..) => op.prec(),
            Expr::Neg(_) => 6,
            _ => 7,
        }
    }

    /// Every cell this expression reads, ranges expanded. `cap` bounds the
    /// expansion so a stray `A1:ZZ100000` cannot blow up the dependency graph.
    pub fn each_ref(&self, cap: usize, f: &mut impl FnMut(usize, usize)) {
        match self {
            Expr::Ref(r) => f(r.row, r.col),
            Expr::Range(a, b) => {
                let (r0, r1) = (a.row.min(b.row), a.row.max(b.row));
                let (c0, c1) = (a.col.min(b.col), a.col.max(b.col));
                if (r1 - r0 + 1).saturating_mul(c1 - c0 + 1) > cap {
                    return;
                }
                for r in r0..=r1 {
                    for c in c0..=c1 {
                        f(r, c);
                    }
                }
            }
            Expr::Neg(e) => e.each_ref(cap, f),
            Expr::Bin(_, a, b) => {
                a.each_ref(cap, f);
                b.each_ref(cap, f);
            }
            Expr::Call(_, args) => {
                for a in args {
                    a.each_ref(cap, f);
                }
            }
            _ => (),
        }
    }

    /// Rewrite every relative reference by (drow, dcol) — the fill-handle and
    /// copy/paste rule. Anchored (`$`) parts stay put; refs pushed off the
    /// sheet become `#REF!`.
    pub fn translate(&self, drow: isize, dcol: isize) -> Expr {
        match self {
            Expr::Ref(r) => match r.translate(drow, dcol) {
                Some(r) => Expr::Ref(r),
                None => Expr::ErrLit(ErrKind::Ref),
            },
            Expr::Range(a, b) => match (a.translate(drow, dcol), b.translate(drow, dcol)) {
                (Some(a), Some(b)) => Expr::Range(a, b),
                _ => Expr::ErrLit(ErrKind::Ref),
            },
            Expr::Neg(e) => Expr::Neg(Box::new(e.translate(drow, dcol))),
            Expr::Bin(op, a, b) => Expr::Bin(
                *op,
                Box::new(a.translate(drow, dcol)),
                Box::new(b.translate(drow, dcol)),
            ),
            Expr::Call(name, args) => Expr::Call(
                name.clone(),
                args.iter().map(|a| a.translate(drow, dcol)).collect(),
            ),
            other => other.clone(),
        }
    }

    /// Render back to formula text (without the leading `=`), inserting only
    /// the parentheses precedence actually requires.
    pub fn to_formula(&self) -> String {
        let mut out = String::new();
        self.write(0, &mut out);
        out
    }

    fn write(&self, min_prec: u8, out: &mut String) {
        let needs = self.prec() < min_prec;
        if needs {
            out.push('(');
        }
        match self {
            Expr::Num(n) => {
                let _ = write!(out, "{}", format_general(*n));
            }
            Expr::Str(s) => {
                let _ = write!(out, "\"{}\"", s.replace('"', "\"\""));
            }
            Expr::Bool(b) => out.push_str(if *b { "TRUE" } else { "FALSE" }),
            Expr::ErrLit(e) => out.push_str(e.text()),
            Expr::Ref(r) => out.push_str(&r.to_a1()),
            Expr::Range(a, b) => {
                let _ = write!(out, "{}:{}", a.to_a1(), b.to_a1());
            }
            Expr::Neg(e) => {
                out.push('-');
                e.write(6, out);
            }
            Expr::Bin(op, a, b) => {
                let p = op.prec();
                // `^` is right associative; everything else is left.
                let (lp, rp) = if *op == BinOp::Pow {
                    (p + 1, p)
                } else {
                    (p, p + 1)
                };
                a.write(lp, out);
                out.push_str(op.text());
                b.write(rp, out);
            }
            Expr::Call(name, args) => {
                let _ = write!(out, "{}(", name);
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    a.write(0, out);
                }
                out.push(')');
            }
        }
        if needs {
            out.push(')');
        }
    }
}

// ---------------------------------------------------------------------------
// parser
// ---------------------------------------------------------------------------

pub fn parse(src: &str) -> Result<Expr, ErrKind> {
    let chars: Vec<char> = src.chars().collect();
    let mut p = Parser { chars, pos: 0 };
    let e = p.parse_compare()?;
    if !p.at_end() {
        return Err(ErrKind::Parse);
    }
    Ok(e)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_ws();
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    fn at_end(&mut self) -> bool {
        self.peek().is_none()
    }

    fn parse_compare(&mut self) -> Result<Expr, ErrKind> {
        let mut lhs = self.parse_concat()?;
        loop {
            let Some(c) = self.peek() else { break };
            let op = match c {
                '=' => {
                    self.pos += 1;
                    BinOp::Eq
                }
                '<' => {
                    self.pos += 1;
                    match self.peek_at(0) {
                        Some('>') => {
                            self.pos += 1;
                            BinOp::Ne
                        }
                        Some('=') => {
                            self.pos += 1;
                            BinOp::Le
                        }
                        _ => BinOp::Lt,
                    }
                }
                '>' => {
                    self.pos += 1;
                    match self.peek_at(0) {
                        Some('=') => {
                            self.pos += 1;
                            BinOp::Ge
                        }
                        _ => BinOp::Gt,
                    }
                }
                _ => break,
            };
            let rhs = self.parse_concat()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_concat(&mut self) -> Result<Expr, ErrKind> {
        let mut lhs = self.parse_add()?;
        while self.peek() == Some('&') {
            self.pos += 1;
            let rhs = self.parse_add()?;
            lhs = Expr::Bin(BinOp::Concat, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_add(&mut self) -> Result<Expr, ErrKind> {
        let mut lhs = self.parse_mul()?;
        while let Some(c) = self.peek() {
            let op = match c {
                '+' => BinOp::Add,
                '-' => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_mul()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ErrKind> {
        let mut lhs = self.parse_pow()?;
        while let Some(c) = self.peek() {
            let op = match c {
                '*' => BinOp::Mul,
                '/' => BinOp::Div,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_pow()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_pow(&mut self) -> Result<Expr, ErrKind> {
        let lhs = self.parse_unary()?;
        if self.peek() == Some('^') {
            self.pos += 1;
            let rhs = self.parse_pow()?;
            return Ok(Expr::Bin(BinOp::Pow, Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ErrKind> {
        match self.peek() {
            Some('-') => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.parse_unary()?)))
            }
            Some('+') => {
                self.pos += 1;
                self.parse_unary()
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Result<Expr, ErrKind> {
        let Some(c) = self.peek() else {
            return Err(ErrKind::Parse);
        };
        if c == '(' {
            self.pos += 1;
            let inner = self.parse_compare()?;
            if self.peek() != Some(')') {
                return Err(ErrKind::Parse);
            }
            self.pos += 1;
            return Ok(inner);
        }
        if c == '"' {
            return self.parse_string();
        }
        if c == '#' {
            return self.parse_error_literal();
        }
        if c.is_ascii_digit() || c == '.' {
            return self.parse_number();
        }
        if c.is_ascii_alphabetic() || c == '$' || c == '_' {
            return self.parse_word();
        }
        Err(ErrKind::Parse)
    }

    fn parse_string(&mut self) -> Result<Expr, ErrKind> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let Some(c) = self.peek_at(0) else {
                return Err(ErrKind::Parse);
            };
            self.pos += 1;
            if c == '"' {
                // "" inside a string is a literal quote
                if self.peek_at(0) == Some('"') {
                    out.push('"');
                    self.pos += 1;
                    continue;
                }
                return Ok(Expr::Str(out));
            }
            out.push(c);
        }
    }

    fn parse_error_literal(&mut self) -> Result<Expr, ErrKind> {
        let start = self.pos;
        // Error literals are the only tokens with '!' or '?' in them.
        while let Some(c) = self.peek_at(0) {
            if c.is_ascii_alphanumeric() || "#/!?0".contains(c) {
                self.pos += 1;
            } else {
                break;
            }
        }
        let word: String = self.chars[start..self.pos].iter().collect();
        for e in [
            ErrKind::Div0,
            ErrKind::Ref,
            ErrKind::Name,
            ErrKind::Value,
            ErrKind::Circ,
            ErrKind::Num,
            ErrKind::Parse,
        ] {
            if word.eq_ignore_ascii_case(e.text()) {
                return Ok(Expr::ErrLit(e));
            }
        }
        Err(ErrKind::Parse)
    }

    fn parse_number(&mut self) -> Result<Expr, ErrKind> {
        let start = self.pos;
        while let Some(c) = self.peek_at(0) {
            if c.is_ascii_digit() || c == '.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        // optional exponent: 1e5, 2.5E-3
        if let Some(c) = self.peek_at(0) {
            if c == 'e' || c == 'E' {
                let save = self.pos;
                self.pos += 1;
                if matches!(self.peek_at(0), Some('+') | Some('-')) {
                    self.pos += 1;
                }
                if matches!(self.peek_at(0), Some(d) if d.is_ascii_digit()) {
                    while matches!(self.peek_at(0), Some(d) if d.is_ascii_digit()) {
                        self.pos += 1;
                    }
                } else {
                    self.pos = save;
                }
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        s.parse::<f64>().map(Expr::Num).map_err(|_| ErrKind::Parse)
    }

    /// A word is a function call, a boolean literal, or a cell reference /
    /// range. `$` may only appear in references.
    fn parse_word(&mut self) -> Result<Expr, ErrKind> {
        let start = self.pos;
        while let Some(c) = self.peek_at(0) {
            if c.is_ascii_alphanumeric() || c == '$' || c == '_' || c == '.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let word: String = self.chars[start..self.pos].iter().collect();
        if word.is_empty() {
            return Err(ErrKind::Parse);
        }

        // function call
        if self.peek() == Some('(') {
            self.pos += 1;
            let mut args = Vec::new();
            if self.peek() != Some(')') {
                loop {
                    args.push(self.parse_compare()?);
                    match self.peek() {
                        Some(',') | Some(';') => {
                            self.pos += 1;
                        }
                        Some(')') => break,
                        _ => return Err(ErrKind::Parse),
                    }
                }
            }
            if self.peek() != Some(')') {
                return Err(ErrKind::Parse);
            }
            self.pos += 1;
            return Ok(Expr::Call(word.to_ascii_uppercase(), args));
        }

        // reference, possibly a range
        if let Some(a) = parse_a1(&word) {
            if self.peek() == Some(':') {
                let save = self.pos;
                self.pos += 1;
                self.skip_ws();
                let s2 = self.pos;
                while let Some(c) = self.peek_at(0) {
                    if c.is_ascii_alphanumeric() || c == '$' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let word2: String = self.chars[s2..self.pos].iter().collect();
                if let Some(b) = parse_a1(&word2) {
                    return Ok(Expr::Range(a, b));
                }
                self.pos = save;
            }
            return Ok(Expr::Ref(a));
        }

        match word.to_ascii_uppercase().as_str() {
            "TRUE" => Ok(Expr::Bool(true)),
            "FALSE" => Ok(Expr::Bool(false)),
            _ => Err(ErrKind::Name),
        }
    }
}

// ---------------------------------------------------------------------------
// evaluation
// ---------------------------------------------------------------------------

/// Where a formula reads its cells from. The sheet implements this over its
/// computed-value cache, so evaluation itself never recurses into other cells.
pub trait CellSource {
    fn cell_value(&mut self, row: usize, col: usize) -> Value;
}

/// A plain map-backed source — used by the tests and by any caller that just
/// wants to evaluate an expression over a handful of cells.
impl<F> CellSource for F
where
    F: FnMut(usize, usize) -> Value,
{
    fn cell_value(&mut self, row: usize, col: usize) -> Value {
        self(row, col)
    }
}

pub fn eval(expr: &Expr, src: &mut dyn CellSource) -> Value {
    match expr {
        Expr::Num(n) => Value::Num(*n),
        Expr::Str(s) => Value::Text(s.clone()),
        Expr::Bool(b) => Value::Bool(*b),
        Expr::ErrLit(e) => Value::Err(*e),
        Expr::Ref(r) => src.cell_value(r.row, r.col),
        // A bare range outside a function has no single value.
        Expr::Range(..) => Value::Err(ErrKind::Value),
        Expr::Neg(e) => match eval(e, src).as_num() {
            Ok(n) => Value::Num(-n),
            Err(e) => Value::Err(e),
        },
        Expr::Bin(op, a, b) => eval_bin(*op, a, b, src),
        Expr::Call(name, args) => eval_call(name, args, src),
    }
}

fn eval_bin(op: BinOp, a: &Expr, b: &Expr, src: &mut dyn CellSource) -> Value {
    let va = eval(a, src);
    let vb = eval(b, src);
    if let Some(e) = va.err().or_else(|| vb.err()) {
        return Value::Err(e);
    }
    match op {
        BinOp::Concat => Value::Text(format!("{}{}", va.to_text(), vb.to_text())),
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord = compare(&va, &vb);
            let r = match op {
                BinOp::Eq => ord == std::cmp::Ordering::Equal,
                BinOp::Ne => ord != std::cmp::Ordering::Equal,
                BinOp::Lt => ord == std::cmp::Ordering::Less,
                BinOp::Le => ord != std::cmp::Ordering::Greater,
                BinOp::Gt => ord == std::cmp::Ordering::Greater,
                _ => ord != std::cmp::Ordering::Less,
            };
            Value::Bool(r)
        }
        _ => {
            let (na, nb) = match (va.as_num(), vb.as_num()) {
                (Ok(x), Ok(y)) => (x, y),
                (Err(e), _) | (_, Err(e)) => return Value::Err(e),
            };
            let n = match op {
                BinOp::Add => na + nb,
                BinOp::Sub => na - nb,
                BinOp::Mul => na * nb,
                BinOp::Div => {
                    if nb == 0.0 {
                        return Value::Err(ErrKind::Div0);
                    }
                    na / nb
                }
                BinOp::Pow => na.powf(nb),
                _ => unreachable!(),
            };
            num(n)
        }
    }
}

fn num(n: f64) -> Value {
    if n.is_finite() {
        Value::Num(n)
    } else {
        Value::Err(ErrKind::Num)
    }
}

/// Excel's cross-type ordering: numbers before text before booleans, text
/// compared case-insensitively. Empty compares as 0 / "" against its peer.
fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let rank = |v: &Value| match v {
        Value::Num(_) | Value::Empty => 0,
        Value::Text(_) => 1,
        Value::Bool(_) => 2,
        Value::Err(_) => 3,
    };
    match (a, b) {
        // Empty against text behaves like an empty string, not like 0.
        (Value::Empty, Value::Text(s)) => "".cmp(s.as_str()),
        (Value::Text(s), Value::Empty) => s.as_str().cmp(""),
        _ => {
            let (ra, rb) = (rank(a), rank(b));
            if ra != rb {
                return ra.cmp(&rb);
            }
            match (a, b) {
                (Value::Text(x), Value::Text(y)) => x
                    .to_ascii_lowercase()
                    .cmp(&y.to_ascii_lowercase()),
                (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
                _ => {
                    let x = a.as_num().unwrap_or(0.0);
                    let y = b.as_num().unwrap_or(0.0);
                    x.partial_cmp(&y).unwrap_or(Ordering::Equal)
                }
            }
        }
    }
}

/// Evaluate one argument into a flat list of values, expanding ranges.
fn arg_values(expr: &Expr, src: &mut dyn CellSource) -> Result<Vec<Value>, ErrKind> {
    match expr {
        Expr::Range(a, b) => {
            let (r0, r1) = (a.row.min(b.row), a.row.max(b.row));
            let (c0, c1) = (a.col.min(b.col), a.col.max(b.col));
            if (r1 - r0 + 1).saturating_mul(c1 - c0 + 1) > 1_000_000 {
                return Err(ErrKind::Num);
            }
            let mut out = Vec::new();
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let v = src.cell_value(r, c);
                    if let Some(e) = v.err() {
                        return Err(e);
                    }
                    out.push(v);
                }
            }
            Ok(out)
        }
        other => {
            let v = eval(other, src);
            match v.err() {
                Some(e) => Err(e),
                None => Ok(vec![v]),
            }
        }
    }
}

/// The numbers in a function's arguments. Ranges contribute only their numeric
/// cells (text and blanks are skipped, as Excel does); a direct argument that
/// is text-but-not-a-number is a `#VALUE!`.
fn arg_nums(args: &[Expr], src: &mut dyn CellSource) -> Result<Vec<f64>, ErrKind> {
    let mut out = Vec::new();
    for a in args {
        let from_range = matches!(a, Expr::Range(..));
        for v in arg_values(a, src)? {
            match &v {
                Value::Num(n) => out.push(*n),
                Value::Empty => (),
                Value::Bool(b) => {
                    if !from_range {
                        out.push(if *b { 1.0 } else { 0.0 })
                    }
                }
                Value::Text(_) => {
                    if from_range {
                        continue;
                    }
                    out.push(v.as_num()?);
                }
                Value::Err(e) => return Err(*e),
            }
        }
    }
    Ok(out)
}

fn eval_call(name: &str, args: &[Expr], src: &mut dyn CellSource) -> Value {
    // IF is lazy: only the taken branch is evaluated, so =IF(A1=0,0,1/A1)
    // does not raise #DIV/0!.
    if name == "IF" {
        if args.len() < 2 || args.len() > 3 {
            return Value::Err(ErrKind::Value);
        }
        let cond = eval(&args[0], src);
        if let Some(e) = cond.err() {
            return Value::Err(e);
        }
        return match cond.as_bool() {
            Ok(true) => eval(&args[1], src),
            Ok(false) => {
                if let Some(e) = args.get(2) {
                    eval(e, src)
                } else {
                    Value::Bool(false)
                }
            }
            Err(e) => Value::Err(e),
        };
    }

    match name {
        "AND" | "OR" => {
            let mut all = true;
            let mut any = false;
            let mut seen = false;
            for a in args {
                let vs = match arg_values(a, src) {
                    Ok(v) => v,
                    Err(e) => return Value::Err(e),
                };
                for v in vs {
                    if matches!(v, Value::Empty) {
                        continue;
                    }
                    match v.as_bool() {
                        Ok(b) => {
                            seen = true;
                            all &= b;
                            any |= b;
                        }
                        Err(e) => return Value::Err(e),
                    }
                }
            }
            if !seen {
                return Value::Err(ErrKind::Value);
            }
            Value::Bool(if name == "AND" { all } else { any })
        }
        "NOT" => {
            if args.len() != 1 {
                return Value::Err(ErrKind::Value);
            }
            match eval(&args[0], src).as_bool() {
                Ok(b) => Value::Bool(!b),
                Err(e) => Value::Err(e),
            }
        }
        "LEN" => {
            if args.len() != 1 {
                return Value::Err(ErrKind::Value);
            }
            let v = eval(&args[0], src);
            match v.err() {
                Some(e) => Value::Err(e),
                None => Value::Num(v.to_text().chars().count() as f64),
            }
        }
        "CONCAT" | "CONCATENATE" => {
            let mut out = String::new();
            for a in args {
                match arg_values(a, src) {
                    Ok(vs) => {
                        for v in vs {
                            out.push_str(&v.to_text());
                        }
                    }
                    Err(e) => return Value::Err(e),
                }
            }
            Value::Text(out)
        }
        "COUNT" => {
            let mut n = 0usize;
            for a in args {
                match arg_values(a, src) {
                    Ok(vs) => n += vs.iter().filter(|v| v.is_num()).count(),
                    Err(e) => return Value::Err(e),
                }
            }
            Value::Num(n as f64)
        }
        "COUNTA" => {
            let mut n = 0usize;
            for a in args {
                match arg_values(a, src) {
                    Ok(vs) => n += vs.iter().filter(|v| !matches!(v, Value::Empty)).count(),
                    Err(e) => return Value::Err(e),
                }
            }
            Value::Num(n as f64)
        }
        _ => {
            let nums = match arg_nums(args, src) {
                Ok(n) => n,
                Err(e) => return Value::Err(e),
            };
            eval_numeric(name, &nums)
        }
    }
}

fn eval_numeric(name: &str, nums: &[f64]) -> Value {
    let one = |n: &[f64]| -> Result<f64, ErrKind> {
        if n.len() == 1 {
            Ok(n[0])
        } else {
            Err(ErrKind::Value)
        }
    };
    let v = match name {
        "SUM" => nums.iter().sum::<f64>(),
        "AVERAGE" | "AVG" => {
            if nums.is_empty() {
                return Value::Err(ErrKind::Div0);
            }
            nums.iter().sum::<f64>() / nums.len() as f64
        }
        "MIN" => {
            if nums.is_empty() {
                0.0
            } else {
                nums.iter().copied().fold(f64::INFINITY, f64::min)
            }
        }
        "MAX" => {
            if nums.is_empty() {
                0.0
            } else {
                nums.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            }
        }
        "ABS" => match one(nums) {
            Ok(n) => n.abs(),
            Err(e) => return Value::Err(e),
        },
        "SQRT" => match one(nums) {
            Ok(n) => {
                if n < 0.0 {
                    return Value::Err(ErrKind::Num);
                }
                n.sqrt()
            }
            Err(e) => return Value::Err(e),
        },
        "ROUND" => match nums.len() {
            1 => nums[0].round(),
            2 => {
                let f = 10f64.powi(nums[1] as i32);
                if !f.is_finite() || f == 0.0 {
                    return Value::Err(ErrKind::Num);
                }
                (nums[0] * f).round() / f
            }
            _ => return Value::Err(ErrKind::Value),
        },
        "INT" => match one(nums) {
            Ok(n) => n.floor(),
            Err(e) => return Value::Err(e),
        },
        "POWER" => {
            if nums.len() != 2 {
                return Value::Err(ErrKind::Value);
            }
            nums[0].powf(nums[1])
        }
        _ => return Value::Err(ErrKind::Name),
    };
    num(v)
}

/// The functions the `fx` menu offers, with a one-line summary each.
pub const FUNCTIONS: &[(&str, &str)] = &[
    ("SUM", "SUM(range) — add numbers"),
    ("AVERAGE", "AVERAGE(range) — arithmetic mean"),
    ("MIN", "MIN(range) — smallest number"),
    ("MAX", "MAX(range) — largest number"),
    ("COUNT", "COUNT(range) — count numbers"),
    ("COUNTA", "COUNTA(range) — count non-empty"),
    ("IF", "IF(test, then, else)"),
    ("AND", "AND(a, b, …) — all true"),
    ("OR", "OR(a, b, …) — any true"),
    ("NOT", "NOT(a) — invert"),
    ("ABS", "ABS(n) — absolute value"),
    ("ROUND", "ROUND(n, digits)"),
    ("SQRT", "SQRT(n) — square root"),
    ("INT", "INT(n) — round down"),
    ("POWER", "POWER(n, e) — n to the e"),
    ("LEN", "LEN(text) — length"),
    ("CONCAT", "CONCAT(a, b, …) — join text"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Evaluate `src` against a fixed set of literal cell values.
    fn ev(src: &str, cells: &[(&str, Value)]) -> Value {
        let mut map: HashMap<(usize, usize), Value> = HashMap::new();
        for (name, v) in cells {
            let r = parse_a1(name).unwrap();
            map.insert((r.row, r.col), v.clone());
        }
        let expr = match parse(src) {
            Ok(e) => e,
            Err(e) => return Value::Err(e),
        };
        let mut f = |row: usize, col: usize| map.get(&(row, col)).cloned().unwrap_or(Value::Empty);
        eval(&expr, &mut f)
    }

    fn n(src: &str) -> Value {
        ev(src, &[])
    }

    // -- numbers, strings, precedence ------------------------------------

    #[test]
    fn arithmetic_precedence() {
        assert_eq!(n("1+2*3"), Value::Num(7.0));
        assert_eq!(n("(1+2)*3"), Value::Num(9.0));
        assert_eq!(n("2^3^2"), Value::Num(512.0)); // right associative
        assert_eq!(n("10-3-2"), Value::Num(5.0)); // left associative
        assert_eq!(n("100/10/2"), Value::Num(5.0));
        assert_eq!(n("-3^2"), Value::Num(9.0)); // unary binds tighter than ^
        assert_eq!(n("-(3^2)"), Value::Num(-9.0));
        assert_eq!(n("--5"), Value::Num(5.0));
        assert_eq!(n("+7"), Value::Num(7.0));
        assert_eq!(n("2.5*4"), Value::Num(10.0));
        assert_eq!(n("1e3+1"), Value::Num(1001.0));
        assert_eq!(n("1.5e-2"), Value::Num(0.015));
    }

    #[test]
    fn whitespace_is_ignored() {
        assert_eq!(n("  1 +  2 * 3 "), Value::Num(7.0));
        assert_eq!(n("SUM( 1 , 2 , 3 )"), Value::Num(6.0));
    }

    #[test]
    fn strings_and_concat() {
        assert_eq!(n("\"ab\"&\"cd\""), Value::Text("abcd".into()));
        assert_eq!(n("\"n=\"&42"), Value::Text("n=42".into()));
        assert_eq!(n("\"a\"\"b\""), Value::Text("a\"b".into()));
        // concat binds looser than +
        assert_eq!(n("1+1&\"x\""), Value::Text("2x".into()));
    }

    #[test]
    fn booleans() {
        assert_eq!(n("TRUE"), Value::Bool(true));
        assert_eq!(n("false"), Value::Bool(false));
        assert_eq!(n("NOT(TRUE)"), Value::Bool(false));
        assert_eq!(n("AND(TRUE,TRUE,FALSE)"), Value::Bool(false));
        assert_eq!(n("AND(TRUE,TRUE)"), Value::Bool(true));
        assert_eq!(n("OR(FALSE,TRUE)"), Value::Bool(true));
        assert_eq!(n("OR(FALSE,FALSE)"), Value::Bool(false));
        assert_eq!(n("TRUE+1"), Value::Num(2.0));
    }

    #[test]
    fn comparisons() {
        assert_eq!(n("1<2"), Value::Bool(true));
        assert_eq!(n("2<=2"), Value::Bool(true));
        assert_eq!(n("3>4"), Value::Bool(false));
        assert_eq!(n("3>=3"), Value::Bool(true));
        assert_eq!(n("1=1"), Value::Bool(true));
        assert_eq!(n("1<>1"), Value::Bool(false));
        assert_eq!(n("\"a\"=\"A\""), Value::Bool(true)); // case insensitive
        assert_eq!(n("\"apple\"<\"banana\""), Value::Bool(true));
        // comparison binds loosest: 1+1=2 is (1+1)=2
        assert_eq!(n("1+1=2"), Value::Bool(true));
    }

    // -- references -------------------------------------------------------

    #[test]
    fn cell_refs_and_absolutes() {
        let cells = &[("A1", Value::Num(2.0)), ("B1", Value::Num(3.0))];
        assert_eq!(ev("A1*B1+4", cells), Value::Num(10.0));
        assert_eq!(ev("$A$1+$B1+A$1", cells), Value::Num(7.0));
        assert_eq!(ev("Z99", cells), Value::Empty); // untouched cell
        assert_eq!(ev("Z99+1", cells), Value::Num(1.0)); // empty is 0 in maths
    }

    #[test]
    fn ref_parsing_roundtrip() {
        let plain = |row, col| CellRef {
            row,
            col,
            abs_row: false,
            abs_col: false,
        };
        assert_eq!(parse_a1("B12"), Some(plain(11, 1)));
        assert_eq!(parse_a1("AA1"), Some(plain(0, 26)));
        assert_eq!(parse_a1("ZZ1").map(|r| r.col), Some(701));
        assert_eq!(ref_name(11, 1), "B12");
        assert_eq!(col_letters(0), "A");
        assert_eq!(col_letters(25), "Z");
        assert_eq!(col_letters(26), "AA");
        assert_eq!(letters_col("AA"), Some(26));
        assert_eq!(parse_a1("A0"), None); // rows are 1-based
        assert_eq!(parse_a1("1A"), None);
        assert_eq!(parse_a1("A"), None);
        let r = parse_a1("$C$7").unwrap();
        assert!(r.abs_row && r.abs_col);
        assert_eq!(r.to_a1(), "$C$7");
    }

    #[test]
    fn ranges_in_functions() {
        let cells = &[
            ("A1", Value::Num(1.0)),
            ("A2", Value::Num(2.0)),
            ("A3", Value::Num(3.0)),
        ];
        assert_eq!(ev("SUM(A1:A3)", cells), Value::Num(6.0));
        assert_eq!(ev("SUM(A3:A1)", cells), Value::Num(6.0)); // reversed
        assert_eq!(ev("AVERAGE(A1:A3)", cells), Value::Num(2.0));
        assert_eq!(ev("MIN(A1:A3)", cells), Value::Num(1.0));
        assert_eq!(ev("MAX(A1:A3)", cells), Value::Num(3.0));
        assert_eq!(ev("COUNT(A1:A3)", cells), Value::Num(3.0));
        assert_eq!(ev("SUM(A1:A3)*2", cells), Value::Num(12.0));
        assert_eq!(ev("SUM($A$1:$A$3)", cells), Value::Num(6.0));
        // a bare range is not a value
        assert_eq!(ev("A1:A3", cells), Value::Err(ErrKind::Value));
    }

    #[test]
    fn ranges_skip_text_and_blanks() {
        let cells = &[
            ("A1", Value::Num(1.0)),
            ("A2", Value::Text("hello".into())),
            ("A4", Value::Num(3.0)),
        ];
        assert_eq!(ev("SUM(A1:A4)", cells), Value::Num(4.0));
        assert_eq!(ev("COUNT(A1:A4)", cells), Value::Num(2.0));
        assert_eq!(ev("COUNTA(A1:A4)", cells), Value::Num(3.0));
        assert_eq!(ev("AVERAGE(A1:A4)", cells), Value::Num(2.0));
    }

    #[test]
    fn two_dimensional_range() {
        let cells = &[
            ("A1", Value::Num(1.0)),
            ("B1", Value::Num(2.0)),
            ("A2", Value::Num(3.0)),
            ("B2", Value::Num(4.0)),
        ];
        assert_eq!(ev("SUM(A1:B2)", cells), Value::Num(10.0));
        assert_eq!(ev("COUNT(A1:B2)", cells), Value::Num(4.0));
    }

    // -- functions --------------------------------------------------------

    #[test]
    fn math_functions() {
        assert_eq!(n("ABS(-3)"), Value::Num(3.0));
        assert_eq!(n("SQRT(16)"), Value::Num(4.0));
        assert_eq!(n("ROUND(2.456,1)"), Value::Num(2.5));
        assert_eq!(n("ROUND(2.5)"), Value::Num(3.0));
        assert_eq!(n("INT(2.9)"), Value::Num(2.0));
        assert_eq!(n("POWER(2,10)"), Value::Num(1024.0));
        assert_eq!(n("MAX(1,5,3)"), Value::Num(5.0));
        assert_eq!(n("MIN(1,5,3)"), Value::Num(1.0));
        assert_eq!(n("SUM(1,2,3)"), Value::Num(6.0));
        assert_eq!(n("sum(1,2)"), Value::Num(3.0)); // case insensitive
    }

    #[test]
    fn text_functions() {
        assert_eq!(n("LEN(\"abcd\")"), Value::Num(4.0));
        assert_eq!(n("LEN(123)"), Value::Num(3.0));
        assert_eq!(n("CONCAT(\"a\",\"b\",\"c\")"), Value::Text("abc".into()));
        assert_eq!(n("CONCAT(1,2)"), Value::Text("12".into()));
    }

    #[test]
    fn if_is_lazy() {
        assert_eq!(n("IF(TRUE,1,2)"), Value::Num(1.0));
        assert_eq!(n("IF(FALSE,1,2)"), Value::Num(2.0));
        assert_eq!(n("IF(1>2,\"yes\",\"no\")"), Value::Text("no".into()));
        // the untaken branch must not be evaluated
        assert_eq!(n("IF(TRUE,1,1/0)"), Value::Num(1.0));
        assert_eq!(n("IF(FALSE,1/0,2)"), Value::Num(2.0));
        // a live divide-by-zero still errors
        assert_eq!(n("IF(FALSE,1,1/0)"), Value::Err(ErrKind::Div0));
        assert_eq!(n("IF(TRUE,1)"), Value::Num(1.0));
        assert_eq!(n("IF(FALSE,1)"), Value::Bool(false));
    }

    #[test]
    fn nested_calls() {
        let cells = &[("A1", Value::Num(4.0)), ("A2", Value::Num(9.0))];
        assert_eq!(ev("SUM(SQRT(A1),SQRT(A2))", cells), Value::Num(5.0));
        assert_eq!(ev("ROUND(AVERAGE(A1:A2),1)", cells), Value::Num(6.5));
        assert_eq!(
            ev("IF(SUM(A1:A2)>10,\"big\",\"small\")", cells),
            Value::Text("big".into())
        );
    }

    // -- errors -----------------------------------------------------------

    #[test]
    fn error_values() {
        assert_eq!(n("1/0"), Value::Err(ErrKind::Div0));
        assert_eq!(n("NOSUCHFN(1)"), Value::Err(ErrKind::Name));
        assert_eq!(n("SQRT(-1)"), Value::Err(ErrKind::Num));
        assert_eq!(n("1+"), Value::Err(ErrKind::Parse));
        assert_eq!(n("(1"), Value::Err(ErrKind::Parse));
        assert_eq!(n("\"abc"), Value::Err(ErrKind::Parse));
        assert_eq!(n("1 2"), Value::Err(ErrKind::Parse));
        assert_eq!(n("\"x\"*2"), Value::Err(ErrKind::Value));
        assert_eq!(
            ErrKind::Div0.text(),
            "#DIV/0!",
            "error spellings are user visible"
        );
        assert_eq!(ErrKind::Ref.text(), "#REF!");
        assert_eq!(ErrKind::Name.text(), "#NAME?");
        assert_eq!(ErrKind::Value.text(), "#VALUE!");
        assert_eq!(ErrKind::Circ.text(), "#CIRC!");
    }

    #[test]
    fn errors_propagate_through_operators() {
        let cells = &[("A1", Value::Err(ErrKind::Div0))];
        assert_eq!(ev("A1+1", cells), Value::Err(ErrKind::Div0));
        assert_eq!(ev("A1&\"x\"", cells), Value::Err(ErrKind::Div0));
        assert_eq!(ev("SUM(A1,1)", cells), Value::Err(ErrKind::Div0));
        assert_eq!(ev("-A1", cells), Value::Err(ErrKind::Div0));
        assert_eq!(ev("SUM(A1:A1)", cells), Value::Err(ErrKind::Div0));
    }

    #[test]
    fn error_literals_roundtrip() {
        assert_eq!(n("#REF!"), Value::Err(ErrKind::Ref));
        assert_eq!(n("#DIV/0!"), Value::Err(ErrKind::Div0));
        assert_eq!(parse("#REF!+1").unwrap().to_formula(), "#REF!+1");
    }

    // -- translation (fill handle / copy-paste) ---------------------------

    #[test]
    fn translate_relative_refs() {
        let e = parse("A1+B2").unwrap();
        assert_eq!(e.translate(1, 0).to_formula(), "A2+B3");
        assert_eq!(e.translate(0, 1).to_formula(), "B1+C2");
        assert_eq!(e.translate(2, 3).to_formula(), "D3+E4");
    }

    #[test]
    fn translate_honours_anchors() {
        let e = parse("$A$1+$A1+A$1+A1").unwrap();
        assert_eq!(e.translate(1, 1).to_formula(), "$A$1+$A2+B$1+B2");
    }

    #[test]
    fn translate_ranges_and_calls() {
        let e = parse("SUM(A1:A3)*C1").unwrap();
        assert_eq!(e.translate(0, 1).to_formula(), "SUM(B1:B3)*D1");
        let e = parse("SUM($A$1:$A$3)").unwrap();
        assert_eq!(e.translate(5, 5).to_formula(), "SUM($A$1:$A$3)");
    }

    #[test]
    fn translate_off_sheet_is_ref_error() {
        let e = parse("A1+1").unwrap();
        assert_eq!(e.translate(-1, 0).to_formula(), "#REF!+1");
        let e = parse("SUM(A1:B2)").unwrap();
        assert_eq!(e.translate(0, -1).to_formula(), "SUM(#REF!)");
    }

    // -- printing ---------------------------------------------------------

    #[test]
    fn printer_keeps_meaning() {
        // Only the parens precedence actually needs survive a round trip.
        for src in [
            "1+2*3",
            "(1+2)*3",
            "2^3^2",
            "(2^3)^2",
            "10-(3-2)",
            "10-3-2",
            "-3^2",
            "SUM(A1:A3)/COUNT(A1:A3)",
            "IF(A1>0,\"y\",\"n\")",
            "\"a\"&B2&\"c\"",
            "1+1=2",
            "$A$1*2",
        ] {
            let e = parse(src).unwrap();
            let printed = e.to_formula();
            let reparsed = parse(&printed).unwrap_or_else(|_| panic!("reparse {printed}"));
            assert_eq!(e, reparsed, "round trip changed meaning for {src}");
        }
    }

    #[test]
    fn printer_drops_redundant_parens() {
        assert_eq!(parse("(1+2)*3").unwrap().to_formula(), "(1+2)*3");
        assert_eq!(parse("1+(2*3)").unwrap().to_formula(), "1+2*3");
        assert_eq!(parse("((A1))").unwrap().to_formula(), "A1");
        assert_eq!(parse("10-(3-2)").unwrap().to_formula(), "10-(3-2)");
    }

    // -- dependency collection -------------------------------------------

    #[test]
    fn each_ref_collects_precedents() {
        let e = parse("SUM(A1:A3)+C5").unwrap();
        let mut got = Vec::new();
        e.each_ref(10_000, &mut |r, c| got.push((r, c)));
        got.sort();
        assert_eq!(got, vec![(0, 0), (1, 0), (2, 0), (4, 2)]);
    }

    #[test]
    fn each_ref_respects_the_cap() {
        let e = parse("SUM(A1:Z1000)").unwrap();
        let mut count = 0;
        e.each_ref(100, &mut |_, _| count += 1);
        assert_eq!(count, 0, "oversized ranges are skipped, not expanded");
    }

    // -- number formatting ------------------------------------------------

    #[test]
    fn general_format_hides_float_noise() {
        assert_eq!(format_general(0.1 + 0.2), "0.3");
        assert_eq!(format_general(1.0), "1");
        assert_eq!(format_general(-4.0), "-4");
        assert_eq!(format_general(0.0), "0");
        assert_eq!(format_general(2.5), "2.5");
        assert_eq!(format_general(1234.5678), "1234.5678");
        assert_eq!(format_general(1.0 / 3.0), "0.3333333333");
        assert_eq!(format_general(1e20), "100000000000000000000");
        assert_eq!(n("0.1+0.2").to_text(), "0.3");
    }

    #[test]
    fn value_text_and_coercion() {
        assert_eq!(Value::Empty.to_text(), "");
        assert_eq!(Value::Bool(true).to_text(), "TRUE");
        assert_eq!(Value::Err(ErrKind::Div0).to_text(), "#DIV/0!");
        assert_eq!(Value::Text("7".into()).as_num(), Ok(7.0));
        assert_eq!(Value::Text("x".into()).as_num(), Err(ErrKind::Value));
        assert_eq!(Value::Empty.as_num(), Ok(0.0));
    }
}
