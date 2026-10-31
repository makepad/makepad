//! The equation language: the subset of EEL2 MilkDrop presets use, compiled
//! to a flat register program and run by a small VM.
//!
//! Every variable is an `f64` slot in a [`Context`]; names are
//! case-insensitive and a name nobody assigned reads 0, as in EEL. A
//! program is a list of [`Op`]s over those slots plus scratch slots for
//! intermediate values, so running one is a loop over a `Vec` with no
//! allocation — per-vertex code runs a few thousand times a frame.
//!
//! Semantics follow MilkDrop's EEL2:
//! * `/` by zero gives 0; `%` is integer modulo (0 when dividing by 0);
//!   `^` is `pow`;
//! * `==`, `equal()` compare within 1e-5; `!=` is its negation;
//! * `if(c, a, b)`, `c ? a : b`, `&&`, `||` evaluate only the branch taken;
//! * `loop(n, body)` and `while(body)` are capped ([`LOOP_CAP`]), so a
//!   preset cannot hang the frame;
//! * `megabuf(i)` / `gmegabuf(i)` are assignable memory, bounded to
//!   [`MEGABUF_LEN`] entries (reads outside read 0, writes are dropped);
//! * `rand(x)` is a float in [0, x) (integer part for x ≥ 1 as MilkDrop's
//!   `int(rand(n))` idiom expects is left to the preset).
//!
//! A statement list evaluates to its last expression; an empty program is
//! valid and does nothing. Compile errors carry a message and the byte
//! offset; a preset whose code does not compile still renders with its base
//! values.

use std::collections::HashMap;

/// Iteration cap for `loop` and `while`.
pub const LOOP_CAP: usize = 1 << 16;
/// Entries in each of `megabuf` and `gmegabuf`.
pub const MEGABUF_LEN: usize = 1 << 20;
/// Slots grow in chunks of this many entries on first write.
const MEGABUF_CHUNK: usize = 1 << 14;

#[derive(Clone, Debug, PartialEq)]
pub struct CompileError {
    pub message: String,
    pub offset: usize,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

/// Sparse growable memory for `megabuf` / `gmegabuf`.
#[derive(Clone, Debug, Default)]
pub struct Megabuf {
    chunks: Vec<Option<Box<[f64]>>>,
}

impl Megabuf {
    fn index(value: f64) -> Option<usize> {
        let i = (value + 0.0001).floor();
        if i >= 0.0 && i < MEGABUF_LEN as f64 {
            Some(i as usize)
        } else {
            None
        }
    }

    pub fn get(&self, index: f64) -> f64 {
        let Some(i) = Self::index(index) else { return 0.0 };
        match self.chunks.get(i / MEGABUF_CHUNK) {
            Some(Some(chunk)) => chunk[i % MEGABUF_CHUNK],
            _ => 0.0,
        }
    }

    pub fn set(&mut self, index: f64, value: f64) {
        let Some(i) = Self::index(index) else { return };
        let chunk = i / MEGABUF_CHUNK;
        if self.chunks.len() <= chunk {
            self.chunks.resize(chunk + 1, None);
        }
        let chunk = self.chunks[chunk].get_or_insert_with(|| vec![0.0; MEGABUF_CHUNK].into_boxed_slice());
        chunk[i % MEGABUF_CHUNK] = value;
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
    }
}

/// A variable namespace and its values: one per code block kind (per frame,
/// per vertex, each wave, each shape). Names resolve to slots at compile
/// time; running reads and writes `values`.
#[derive(Clone, Debug, Default)]
pub struct Context {
    names: HashMap<String, usize>,
    pub values: Vec<f64>,
    pub megabuf: Megabuf,
    /// xorshift state for `rand`.
    rng: u64,
}

impl Context {
    pub fn new(seed: u64) -> Self {
        Self { rng: seed | 1, ..Default::default() }
    }

    /// The slot for `name` (lower-cased), created at 0 when new.
    pub fn slot(&mut self, name: &str) -> usize {
        let name = name.to_ascii_lowercase();
        if let Some(&slot) = self.names.get(&name) {
            return slot;
        }
        let slot = self.values.len();
        self.values.push(0.0);
        self.names.insert(name, slot);
        slot
    }

    /// The slot for `name` if the context has one.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.names.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn get(&self, name: &str) -> f64 {
        self.find(name).map(|s| self.values[s]).unwrap_or(0.0)
    }

    pub fn set(&mut self, name: &str, value: f64) {
        let slot = self.slot(name);
        self.values[slot] = value;
    }

    fn next_rand(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---- lexer ----

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    /// Operators and punctuation, longest match first.
    Op(&'static str),
    End,
}

const OPS: &[&str] = &[
    "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|=", "<", ">", "+", "-", "*",
    "/", "%", "^", "=", "!", "(", ")", ",", ";", "?", ":", "&", "|", "[", "]",
];

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, CompileError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Comments: `//` to end of line, `/* */` blocks.
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        let start = i;
        if c.is_ascii_digit() || (c == b'.' && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit())) {
            if c == b'0' && matches!(bytes.get(i + 1), Some(b'x' | b'X')) {
                i += 2;
                let hs = i;
                while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let v = u64::from_str_radix(&src[hs..i], 16).unwrap_or(0) as f64;
                out.push((Tok::Num(v), start));
                continue;
            }
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
                let save = i;
                i += 1;
                if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
                    i += 1;
                }
                if i < bytes.len() && bytes[i].is_ascii_digit() {
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            // `1.2.3` style junk: take the longest prefix that parses.
            let mut text = &src[start..i];
            while !text.is_empty() && text.parse::<f64>().is_err() {
                text = &text[..text.len() - 1];
            }
            out.push((Tok::Num(text.parse().unwrap_or(0.0)), start));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c == b'#' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.') {
                i += 1;
            }
            let name = src[start..i].to_ascii_lowercase();
            match name.as_str() {
                "$pi" => out.push((Tok::Num(std::f64::consts::PI), start)),
                "$e" => out.push((Tok::Num(std::f64::consts::E), start)),
                "$phi" => out.push((Tok::Num(1.618_033_988_749_895), start)),
                _ => out.push((Tok::Ident(name), start)),
            }
            continue;
        }
        let rest = &src[i..];
        match OPS.iter().find(|op| rest.starts_with(**op)) {
            Some(op) => {
                out.push((Tok::Op(op), start));
                i += op.len();
            }
            None => {
                // Stray characters (`@`, backticks, non-ASCII) are skipped as
                // EEL skips what it does not know rather than failing a preset.
                i += src[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            }
        }
    }
    out.push((Tok::End, bytes.len()));
    Ok(out)
}

// ---- AST ----

/// The parsed form of a code block, shared with the GPU backend
/// ([`crate::eel_gpu`]).
#[derive(Clone, Debug)]
pub(crate) enum Expr {
    Num(f64),
    Var(String),
    /// `megabuf(i)` (false) or `gmegabuf(i)` (true).
    Mem(bool, Box<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// `op` is "=" or a compound operator.
    Assign(&'static str, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Loop(Box<Expr>, Box<Expr>),
    While(Box<Expr>),
    Call(String, Vec<Expr>),
    Seq(Vec<Expr>),
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }
    fn offset(&self) -> usize {
        self.toks[self.pos].1
    }
    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }
    fn eat(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn err<T>(&self, message: &str) -> Result<T, CompileError> {
        Err(CompileError { message: message.to_string(), offset: self.offset() })
    }

    /// `a; b; c` — statements until `)`, `,`, or the end.
    fn sequence(&mut self) -> Result<Expr, CompileError> {
        let mut items = Vec::new();
        loop {
            while self.eat(";") {}
            if matches!(self.peek(), Tok::End) || self.is_op(")") || self.is_op(",") {
                break;
            }
            items.push(self.assignment()?);
            if !self.eat(";") {
                if matches!(self.peek(), Tok::End) || self.is_op(")") || self.is_op(",") {
                    break;
                }
                // Missing separator between statements: EEL tolerates
                // `a=1 b=2` in some presets; treat it as a separator.
            }
        }
        Ok(match items.len() {
            0 => Expr::Num(0.0),
            1 => items.pop().unwrap(),
            _ => Expr::Seq(items),
        })
    }

    fn assignment(&mut self) -> Result<Expr, CompileError> {
        let left = self.ternary()?;
        for op in ["=", "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|="] {
            if self.is_op(op) {
                let op_offset = self.offset();
                self.pos += 1;
                let right = self.assignment()?;
                return match left {
                    Expr::Var(_) | Expr::Mem(..) => Ok(Expr::Assign(op, Box::new(left), Box::new(right))),
                    _ => Err(CompileError { message: "cannot assign to this".into(), offset: op_offset }),
                };
            }
        }
        Ok(left)
    }

    fn ternary(&mut self) -> Result<Expr, CompileError> {
        let cond = self.logic_or()?;
        if self.eat("?") {
            let a = self.assignment()?;
            let b = if self.eat(":") { self.assignment()? } else { Expr::Num(0.0) };
            return Ok(Expr::If(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Ok(cond)
    }

    fn logic_or(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.logic_and()?;
        while self.eat("||") {
            let right = self.logic_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn logic_and(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.bit_or()?;
        while self.eat("&&") {
            let right = self.bit_or()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn bit_or(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.bit_and()?;
        while self.eat("|") {
            let right = self.bit_and()?;
            left = Expr::Binary("|", Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn bit_and(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.compare()?;
        while self.eat("&") {
            let right = self.compare()?;
            left = Expr::Binary("&", Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn compare(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.additive()?;
        loop {
            let op = match self.peek() {
                Tok::Op(o) if matches!(*o, "==" | "!=" | "<" | ">" | "<=" | ">=") => *o,
                _ => break,
            };
            self.pos += 1;
            let right = self.additive()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn additive(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Op(o) if matches!(*o, "+" | "-") => *o,
                _ => break,
            };
            self.pos += 1;
            let right = self.multiplicative()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Op(o) if matches!(*o, "*" | "/" | "%") => *o,
                _ => break,
            };
            self.pos += 1;
            let right = self.unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, CompileError> {
        if self.eat("-") {
            return Ok(Expr::Unary("-", Box::new(self.unary()?)));
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok(Expr::Unary("!", Box::new(self.unary()?)));
        }
        self.power()
    }

    /// `^` binds tighter than unary minus on its right (`-2^2` = -4) and is
    /// right-associative.
    fn power(&mut self) -> Result<Expr, CompileError> {
        let base = self.primary()?;
        if self.eat("^") {
            let exp = self.unary()?;
            return Ok(Expr::Binary("^", Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Expr, CompileError> {
        match self.peek().clone() {
            Tok::Num(v) => {
                self.pos += 1;
                Ok(Expr::Num(v))
            }
            Tok::Op("(") => {
                self.pos += 1;
                let inner = self.sequence()?;
                if !self.eat(")") {
                    return self.err("expected ')'");
                }
                Ok(inner)
            }
            Tok::Ident(name) => {
                self.pos += 1;
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.is_op(")") {
                        loop {
                            args.push(self.sequence()?);
                            if !self.eat(",") {
                                break;
                            }
                        }
                    }
                    if !self.eat(")") {
                        return self.err("expected ')' after arguments");
                    }
                    return self.call(name, args);
                }
                // `megabuf[i]` bracket form.
                if (name == "megabuf" || name == "gmegabuf") && self.eat("[") {
                    let index = self.sequence()?;
                    if !self.eat("]") {
                        return self.err("expected ']'");
                    }
                    return Ok(Expr::Mem(name == "gmegabuf", Box::new(index)));
                }
                Ok(Expr::Var(name))
            }
            Tok::End => self.err("unexpected end of code"),
            Tok::Op(op) => self.err(&format!("unexpected '{op}'")),
        }
    }

    fn call(&mut self, name: String, mut args: Vec<Expr>) -> Result<Expr, CompileError> {
        let arity = |n: usize, args: &Vec<Expr>| -> Result<(), CompileError> {
            if args.len() == n {
                Ok(())
            } else {
                Err(CompileError { message: format!("{name}() takes {n} arguments"), offset: self.offset() })
            }
        };
        match name.as_str() {
            "if" => {
                arity(3, &args)?;
                let c = args.remove(0);
                let a = args.remove(0);
                let b = args.remove(0);
                Ok(Expr::If(Box::new(c), Box::new(a), Box::new(b)))
            }
            "loop" => {
                arity(2, &args)?;
                let n = args.remove(0);
                let body = args.remove(0);
                Ok(Expr::Loop(Box::new(n), Box::new(body)))
            }
            "while" => {
                arity(1, &args)?;
                Ok(Expr::While(Box::new(args.remove(0))))
            }
            "megabuf" | "gmegabuf" => {
                arity(1, &args)?;
                Ok(Expr::Mem(name == "gmegabuf", Box::new(args.remove(0))))
            }
            "assign" => {
                arity(2, &args)?;
                let target = args.remove(0);
                let value = args.remove(0);
                match target {
                    Expr::Var(_) | Expr::Mem(..) => Ok(Expr::Assign("=", Box::new(target), Box::new(value))),
                    _ => self.err("assign() needs a variable"),
                }
            }
            "exec2" | "exec3" => Ok(Expr::Seq(args)),
            _ => Ok(Expr::Call(name, args)),
        }
    }
}

/// Parse `src` to its syntax tree without compiling it.
pub(crate) fn parse(src: &str) -> Result<Expr, CompileError> {
    let toks = lex(src)?;
    let mut parser = Parser { toks, pos: 0 };
    let ast = parser.sequence()?;
    if !matches!(parser.peek(), Tok::End) {
        return parser.err("unexpected text");
    }
    Ok(ast)
}

// ---- program ----

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fn1 {
    Neg,
    Not,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sqrt,
    Sqr,
    Abs,
    Sign,
    Exp,
    Log,
    Log10,
    Int,
    Floor,
    Ceil,
    Bnot,
    InvSqrt,
    Rand,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fn2 {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    BitAnd,
    BitOr,
    Min,
    Max,
    Atan2,
    Above,
    Below,
    Band,
    Bor,
    Sigmoid,
    Fmod,
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Const { dst: u32, value: f64 },
    Mov { dst: u32, src: u32 },
    Un { f: Fn1, dst: u32, a: u32 },
    Bin { f: Fn2, dst: u32, a: u32, b: u32 },
    MemGet { global: bool, dst: u32, index: u32 },
    MemSet { global: bool, index: u32, src: u32 },
    /// Jump to `target` when slot `cond` is zero (EEL truthiness: |x| < 1e-5
    /// is false).
    JumpIfFalse { cond: u32, target: u32 },
    Jump { target: u32 },
    /// Loop counter: `counter = clamp(int(n), 0, LOOP_CAP)`.
    LoopInit { counter: u32, n: u32 },
    /// `if counter <= 0 goto target; counter -= 1`.
    LoopNext { counter: u32, target: u32 },
}

/// A compiled code block, bound to the [`Context`] it was compiled against.
#[derive(Clone, Debug, Default)]
pub struct Program {
    ops: Vec<Op>,
    /// Slot the program's value lands in.
    result: Option<u32>,
}

impl Program {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Compile `src` against `ctx`, creating the variables it names.
    pub fn compile(src: &str, ctx: &mut Context) -> Result<Program, CompileError> {
        let ast = parse(src)?;
        let mut compiler = Compiler { ctx, ops: Vec::new(), scratch_base: 0, scratch_used: 0, scratch_max: 0 };
        // Scratch slots are numbered after every variable the program can
        // name, so collect names first.
        compiler.declare(&ast);
        compiler.scratch_base = compiler.ctx.values.len();
        let result = compiler.expr(&ast);
        // Reserve the scratch region so variables named by programs compiled
        // later land past it, never on it.
        let end = compiler.scratch_base + compiler.scratch_max;
        let ops = compiler.ops;
        if ctx.values.len() < end {
            ctx.values.resize(end, 0.0);
        }
        Ok(Program { ops, result: Some(result) })
    }

    /// Run against `ctx` (the context this program was compiled for, or a
    /// clone of it) and `global` (shared `gmegabuf`). Returns the value of
    /// the last statement.
    pub fn run(&self, ctx: &mut Context, global: &mut Megabuf) -> f64 {
        if self.ops.is_empty() {
            return 0.0;
        }
        let mut pc = 0usize;
        let ops = &self.ops;
        while pc < ops.len() {
            match ops[pc] {
                Op::Const { dst, value } => ctx.values[dst as usize] = value,
                Op::Mov { dst, src } => ctx.values[dst as usize] = ctx.values[src as usize],
                Op::Un { f, dst, a } => {
                    let x = ctx.values[a as usize];
                    let v = match f {
                        Fn1::Neg => -x,
                        Fn1::Not => bool_f(x.abs() < 1e-5),
                        Fn1::Sin => x.sin(),
                        Fn1::Cos => x.cos(),
                        Fn1::Tan => x.tan(),
                        Fn1::Asin => x.clamp(-1.0, 1.0).asin(),
                        Fn1::Acos => x.clamp(-1.0, 1.0).acos(),
                        Fn1::Atan => x.atan(),
                        Fn1::Sqrt => x.abs().sqrt(),
                        Fn1::Sqr => x * x,
                        Fn1::Abs => x.abs(),
                        Fn1::Sign => {
                            if x > 0.0 {
                                1.0
                            } else if x < 0.0 {
                                -1.0
                            } else {
                                0.0
                            }
                        }
                        Fn1::Exp => x.exp(),
                        Fn1::Log => {
                            if x > 0.0 {
                                x.ln()
                            } else {
                                0.0
                            }
                        }
                        Fn1::Log10 => {
                            if x > 0.0 {
                                x.log10()
                            } else {
                                0.0
                            }
                        }
                        Fn1::Int | Fn1::Floor => x.floor(),
                        Fn1::Ceil => x.ceil(),
                        Fn1::Bnot => bool_f(x.abs() < 1e-5),
                        Fn1::InvSqrt => {
                            let s = x.abs().sqrt();
                            if s > 0.0 {
                                1.0 / s
                            } else {
                                0.0
                            }
                        }
                        Fn1::Rand => ctx.next_rand() * x.max(0.0),
                    };
                    ctx.values[dst as usize] = v;
                }
                Op::Bin { f, dst, a, b } => {
                    let x = ctx.values[a as usize];
                    let y = ctx.values[b as usize];
                    ctx.values[dst as usize] = binary(f, x, y);
                }
                Op::MemGet { global: g, dst, index } => {
                    let i = ctx.values[index as usize];
                    ctx.values[dst as usize] = if g { global.get(i) } else { ctx.megabuf.get(i) };
                }
                Op::MemSet { global: g, index, src } => {
                    let i = ctx.values[index as usize];
                    let v = ctx.values[src as usize];
                    if g {
                        global.set(i, v)
                    } else {
                        ctx.megabuf.set(i, v)
                    }
                }
                Op::JumpIfFalse { cond, target } => {
                    if ctx.values[cond as usize].abs() < 1e-5 {
                        pc = target as usize;
                        continue;
                    }
                }
                Op::Jump { target } => {
                    pc = target as usize;
                    continue;
                }
                Op::LoopInit { counter, n } => {
                    let n = ctx.values[n as usize];
                    let n = if n.is_finite() { n.floor().clamp(0.0, LOOP_CAP as f64) } else { 0.0 };
                    ctx.values[counter as usize] = n;
                }
                Op::LoopNext { counter, target } => {
                    let c = ctx.values[counter as usize];
                    if c <= 0.0 {
                        pc = target as usize;
                        continue;
                    }
                    ctx.values[counter as usize] = c - 1.0;
                }
            }
            pc += 1;
        }
        self.result.map(|r| ctx.values.get(r as usize).copied().unwrap_or(0.0)).unwrap_or(0.0)
    }

}

fn bool_f(b: bool) -> f64 {
    if b {
        1.0
    } else {
        0.0
    }
}

fn binary(f: Fn2, x: f64, y: f64) -> f64 {
    match f {
        Fn2::Add => x + y,
        Fn2::Sub => x - y,
        Fn2::Mul => x * y,
        Fn2::Div => {
            if y == 0.0 {
                0.0
            } else {
                x / y
            }
        }
        Fn2::Mod => {
            let b = y.trunc() as i64;
            if b == 0 {
                0.0
            } else {
                (x.trunc() as i64 % b) as f64
            }
        }
        Fn2::Fmod => {
            if y == 0.0 {
                0.0
            } else {
                x % y
            }
        }
        Fn2::Pow => {
            let v = x.powf(y);
            if v.is_finite() {
                v
            } else {
                0.0
            }
        }
        Fn2::Eq => bool_f((x - y).abs() < 1e-5),
        Fn2::Ne => bool_f((x - y).abs() >= 1e-5),
        Fn2::Lt | Fn2::Below => bool_f(x < y),
        Fn2::Gt | Fn2::Above => bool_f(x > y),
        Fn2::Le => bool_f(x <= y),
        Fn2::Ge => bool_f(x >= y),
        Fn2::BitAnd => ((x as i64) & (y as i64)) as f64,
        Fn2::BitOr => ((x as i64) | (y as i64)) as f64,
        Fn2::Min => x.min(y),
        Fn2::Max => x.max(y),
        Fn2::Atan2 => x.atan2(y),
        Fn2::Band => bool_f(x.abs() >= 1e-5 && y.abs() >= 1e-5),
        Fn2::Bor => bool_f(x.abs() >= 1e-5 || y.abs() >= 1e-5),
        Fn2::Sigmoid => {
            let t = 1.0 + (-x * y).exp();
            if t.abs() > 1e-5 {
                1.0 / t
            } else {
                0.0
            }
        }
    }
}

struct Compiler<'a> {
    ctx: &'a mut Context,
    ops: Vec<Op>,
    scratch_base: usize,
    scratch_used: usize,
    scratch_max: usize,
}

impl Compiler<'_> {
    /// Create every variable the program names before scratch is laid out.
    fn declare(&mut self, e: &Expr) {
        match e {
            Expr::Var(name) => {
                self.ctx.slot(name);
            }
            Expr::Num(_) => {}
            Expr::Mem(_, a) | Expr::Unary(_, a) | Expr::While(a) => self.declare(a),
            Expr::Binary(_, a, b) | Expr::Assign(_, a, b) | Expr::And(a, b) | Expr::Or(a, b) | Expr::Loop(a, b) => {
                self.declare(a);
                self.declare(b);
            }
            Expr::If(a, b, c) => {
                self.declare(a);
                self.declare(b);
                self.declare(c);
            }
            Expr::Call(_, args) | Expr::Seq(args) => args.iter().for_each(|a| self.declare(a)),
        }
    }

    fn temp(&mut self) -> u32 {
        let slot = self.scratch_base + self.scratch_used;
        self.scratch_used += 1;
        self.scratch_max = self.scratch_max.max(self.scratch_used);
        slot as u32
    }

    fn var(&mut self, name: &str) -> u32 {
        self.ctx.slot(name) as u32
    }

    fn here(&self) -> u32 {
        self.ops.len() as u32
    }

    fn patch(&mut self, at: u32, target: u32) {
        match &mut self.ops[at as usize] {
            Op::JumpIfFalse { target: t, .. } | Op::Jump { target: t } | Op::LoopNext { target: t, .. } => {
                *t = target
            }
            _ => {}
        }
    }

    /// Compile `e`; the returned slot holds its value.
    fn expr(&mut self, e: &Expr) -> u32 {
        match e {
            Expr::Num(v) => {
                let dst = self.temp();
                self.ops.push(Op::Const { dst, value: *v });
                dst
            }
            Expr::Var(name) => self.var(name),
            Expr::Mem(global, index) => {
                let index = self.expr(index);
                let dst = self.temp();
                self.ops.push(Op::MemGet { global: *global, dst, index });
                dst
            }
            Expr::Unary(op, a) => {
                let a = self.expr(a);
                let dst = self.temp();
                let f = if *op == "-" { Fn1::Neg } else { Fn1::Not };
                self.ops.push(Op::Un { f, dst, a });
                dst
            }
            Expr::Binary(op, a, b) => {
                let a = self.expr(a);
                let b = self.expr(b);
                let dst = self.temp();
                let f = match *op {
                    "+" => Fn2::Add,
                    "-" => Fn2::Sub,
                    "*" => Fn2::Mul,
                    "/" => Fn2::Div,
                    "%" => Fn2::Mod,
                    "^" => Fn2::Pow,
                    "==" => Fn2::Eq,
                    "!=" => Fn2::Ne,
                    "<" => Fn2::Lt,
                    ">" => Fn2::Gt,
                    "<=" => Fn2::Le,
                    ">=" => Fn2::Ge,
                    "&" => Fn2::BitAnd,
                    _ => Fn2::BitOr,
                };
                self.ops.push(Op::Bin { f, dst, a, b });
                dst
            }
            Expr::Assign(op, target, value) => {
                let v = self.expr(value);
                let result = match target.as_ref() {
                    Expr::Var(name) => {
                        let slot = self.var(name);
                        if *op == "=" {
                            self.ops.push(Op::Mov { dst: slot, src: v });
                        } else {
                            let f = compound(op);
                            self.ops.push(Op::Bin { f, dst: slot, a: slot, b: v });
                        }
                        slot
                    }
                    Expr::Mem(global, index) => {
                        let index = self.expr(index);
                        let src = if *op == "=" {
                            v
                        } else {
                            let cur = self.temp();
                            self.ops.push(Op::MemGet { global: *global, dst: cur, index });
                            let dst = self.temp();
                            self.ops.push(Op::Bin { f: compound(op), dst, a: cur, b: v });
                            dst
                        };
                        self.ops.push(Op::MemSet { global: *global, index, src });
                        src
                    }
                    _ => v,
                };
                result
            }
            Expr::If(c, a, b) => {
                let dst = self.temp();
                let c = self.expr(c);
                let jf = self.here();
                self.ops.push(Op::JumpIfFalse { cond: c, target: 0 });
                let av = self.expr(a);
                self.ops.push(Op::Mov { dst, src: av });
                let jend = self.here();
                self.ops.push(Op::Jump { target: 0 });
                let else_at = self.here();
                self.patch(jf, else_at);
                let bv = self.expr(b);
                self.ops.push(Op::Mov { dst, src: bv });
                let end = self.here();
                self.patch(jend, end);
                dst
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                // Result is 0/1; the right side runs only when needed.
                let is_and = matches!(e, Expr::And(..));
                let dst = self.temp();
                let av = self.expr(a);
                let truth = self.temp();
                let zero = self.temp();
                self.ops.push(Op::Const { dst: zero, value: 0.0 });
                self.ops.push(Op::Bin { f: Fn2::Ne, dst: truth, a: av, b: zero });
                self.ops.push(Op::Mov { dst, src: truth });
                if is_and {
                    let jf = self.here();
                    self.ops.push(Op::JumpIfFalse { cond: truth, target: 0 });
                    let bv = self.expr(b);
                    self.ops.push(Op::Bin { f: Fn2::Ne, dst, a: bv, b: zero });
                    let end = self.here();
                    self.patch(jf, end);
                } else {
                    let not = self.temp();
                    self.ops.push(Op::Un { f: Fn1::Not, dst: not, a: truth });
                    let jf = self.here();
                    self.ops.push(Op::JumpIfFalse { cond: not, target: 0 });
                    let bv = self.expr(b);
                    self.ops.push(Op::Bin { f: Fn2::Ne, dst, a: bv, b: zero });
                    let end = self.here();
                    self.patch(jf, end);
                }
                dst
            }
            Expr::Loop(n, body) => {
                let nv = self.expr(n);
                let counter = self.temp();
                self.ops.push(Op::LoopInit { counter, n: nv });
                let top = self.here();
                self.ops.push(Op::LoopNext { counter, target: 0 });
                self.expr(body);
                self.ops.push(Op::Jump { target: top });
                let end = self.here();
                self.patch(top, end);
                let dst = self.temp();
                self.ops.push(Op::Const { dst, value: 0.0 });
                dst
            }
            Expr::While(body) => {
                // `while(body)`: run body, repeat while its value is true,
                // at most LOOP_CAP times.
                let counter = self.temp();
                let cap = self.temp();
                self.ops.push(Op::Const { dst: cap, value: LOOP_CAP as f64 });
                self.ops.push(Op::LoopInit { counter, n: cap });
                let top = self.here();
                let next = self.here();
                self.ops.push(Op::LoopNext { counter, target: 0 });
                let v = self.expr(body);
                let jf = self.here();
                self.ops.push(Op::JumpIfFalse { cond: v, target: 0 });
                self.ops.push(Op::Jump { target: top });
                let end = self.here();
                self.patch(next, end);
                self.patch(jf, end);
                let dst = self.temp();
                self.ops.push(Op::Const { dst, value: 0.0 });
                dst
            }
            Expr::Seq(items) => {
                let mut last = None;
                for item in items {
                    last = Some(self.expr(item));
                }
                match last {
                    Some(slot) => slot,
                    None => {
                        let dst = self.temp();
                        self.ops.push(Op::Const { dst, value: 0.0 });
                        dst
                    }
                }
            }
            Expr::Call(name, args) => self.call(name, args),
        }
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> u32 {
        let slots: Vec<u32> = args.iter().map(|a| self.expr(a)).collect();
        let one = match name {
            "sin" => Some(Fn1::Sin),
            "cos" => Some(Fn1::Cos),
            "tan" => Some(Fn1::Tan),
            "asin" => Some(Fn1::Asin),
            "acos" => Some(Fn1::Acos),
            "atan" => Some(Fn1::Atan),
            "sqrt" => Some(Fn1::Sqrt),
            "sqr" => Some(Fn1::Sqr),
            "abs" => Some(Fn1::Abs),
            "sign" => Some(Fn1::Sign),
            "exp" => Some(Fn1::Exp),
            "log" => Some(Fn1::Log),
            "log10" => Some(Fn1::Log10),
            "int" => Some(Fn1::Int),
            "floor" => Some(Fn1::Floor),
            "ceil" => Some(Fn1::Ceil),
            "bnot" => Some(Fn1::Bnot),
            "invsqrt" => Some(Fn1::InvSqrt),
            "rand" => Some(Fn1::Rand),
            _ => None,
        };
        let two = match name {
            "min" => Some(Fn2::Min),
            "max" => Some(Fn2::Max),
            "atan2" => Some(Fn2::Atan2),
            "pow" => Some(Fn2::Pow),
            "above" => Some(Fn2::Above),
            "below" => Some(Fn2::Below),
            "equal" => Some(Fn2::Eq),
            "band" => Some(Fn2::Band),
            "bor" => Some(Fn2::Bor),
            "sigmoid" => Some(Fn2::Sigmoid),
            "fmod" => Some(Fn2::Fmod),
            _ => None,
        };
        let dst = self.temp();
        match (one, two, slots.as_slice()) {
            (Some(f), _, [a, ..]) => self.ops.push(Op::Un { f, dst, a: *a }),
            (_, Some(f), [a, b, ..]) => self.ops.push(Op::Bin { f, dst, a: *a, b: *b }),
            // Unknown functions and wrong arities read as 0, as EEL
            // evaluates an unresolved call to nothing.
            _ => self.ops.push(Op::Const { dst, value: 0.0 }),
        }
        dst
    }
}

fn compound(op: &str) -> Fn2 {
    match op {
        "+=" => Fn2::Add,
        "-=" => Fn2::Sub,
        "*=" => Fn2::Mul,
        "/=" => Fn2::Div,
        "%=" => Fn2::Mod,
        "^=" => Fn2::Pow,
        "&=" => Fn2::BitAnd,
        _ => Fn2::BitOr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> (f64, Context) {
        let mut ctx = Context::new(7);
        let program = Program::compile(src, &mut ctx).expect("compiles");
        let mut global = Megabuf::default();
        let v = program.run(&mut ctx, &mut global);
        (v, ctx)
    }

    #[test]
    fn arithmetic_follows_eel() {
        assert_eq!(eval("1+2*3").0, 7.0);
        assert_eq!(eval("2^3^2").0, 512.0);
        assert_eq!(eval("-2^2").0, -4.0);
        assert_eq!(eval("5/0").0, 0.0);
        assert_eq!(eval("7%3").0, 1.0);
        assert_eq!(eval("7.9%3.2").0, 1.0);
        assert_eq!(eval("5%0").0, 0.0);
        assert_eq!(eval("1 == 1.000001").0, 1.0);
        assert_eq!(eval("1 != 1.000001").0, 0.0);
        assert_eq!(eval("above(2,1) + below(2,1)*10 + equal(3,3)*100").0, 101.0);
        assert_eq!(eval("int(-1.5)").0, -2.0);
        assert_eq!(eval("sqr(3)+sqrt(16)").0, 13.0);
        assert_eq!(eval("band(1,0)+bor(1,0)*2+bnot(0)*4").0, 6.0);
        assert_eq!(eval("$pi").0, std::f64::consts::PI);
        assert_eq!(eval(".5+0x10").0, 16.5);
    }

    #[test]
    fn variables_are_case_insensitive_and_start_at_zero() {
        let (v, ctx) = eval("Zoom = zoom + 1; ZOOM += 2; q1 = undefined_thing");
        assert_eq!(v, 0.0);
        assert_eq!(ctx.get("zoom"), 3.0);
        assert_eq!(ctx.get("q1"), 0.0);
    }

    #[test]
    fn branches_run_only_when_taken() {
        let (_, ctx) = eval("if(1, a=1, b=1); c = 0 ? (d=1) : (e=2); 0 && (f=1); 1 || (g=1); 1 && (h=1)");
        assert_eq!(ctx.get("a"), 1.0);
        assert_eq!(ctx.get("b"), 0.0);
        assert_eq!(ctx.get("c"), 2.0);
        assert_eq!(ctx.get("d"), 0.0);
        assert_eq!(ctx.get("e"), 2.0);
        assert_eq!(ctx.get("f"), 0.0);
        assert_eq!(ctx.get("g"), 0.0);
        assert_eq!(ctx.get("h"), 1.0);
    }

    #[test]
    fn loops_are_bounded() {
        let (_, ctx) = eval("n=0; loop(10, n+=1); m=0; while(m+=1; m<5); k=0; while(k+=1; 1)");
        assert_eq!(ctx.get("n"), 10.0);
        assert_eq!(ctx.get("m"), 5.0);
        assert_eq!(ctx.get("k"), LOOP_CAP as f64);
        let (_, ctx) = eval("n=0; loop(-3, n+=1)");
        assert_eq!(ctx.get("n"), 0.0);
    }

    #[test]
    fn megabuf_reads_and_writes() {
        let (v, _) = eval("megabuf(5) = 3; megabuf(5) += 2; megabuf(5)");
        assert_eq!(v, 5.0);
        let (v, _) = eval("megabuf(-1) = 3; megabuf(99999999) = 4; megabuf(-1) + megabuf(99999999)");
        assert_eq!(v, 0.0);
        let mut ctx = Context::new(1);
        let mut global = Megabuf::default();
        Program::compile("gmegabuf(2) = 9", &mut ctx).unwrap().run(&mut ctx, &mut global);
        let mut other = Context::new(2);
        assert_eq!(Program::compile("gmegabuf(2)", &mut other).unwrap().run(&mut other, &mut global), 9.0);
    }

    #[test]
    fn comments_and_empty_code() {
        assert_eq!(eval("").0, 0.0);
        assert_eq!(eval("// nothing\n").0, 0.0);
        assert_eq!(eval("a = 1; // b = 2\nc = 3 /* d = 4 */").1.get("b"), 0.0);
        assert_eq!(eval("a = 1;;;b=2;").1.get("b"), 2.0);
    }

    #[test]
    fn rand_is_in_range() {
        let (_, ctx) = eval("lo=100; hi=-1; loop(1000, r=rand(10); lo=min(lo,r); hi=max(hi,r))");
        assert!(ctx.get("lo") >= 0.0 && ctx.get("hi") < 10.0 && ctx.get("hi") > 5.0);
    }

    #[test]
    fn errors_point_at_the_problem() {
        let mut ctx = Context::new(1);
        let err = Program::compile("a = (1 + 2", &mut ctx).unwrap_err();
        assert!(err.message.contains("')'"));
        assert!(Program::compile("3 = 4", &mut ctx).is_err());
    }

    #[test]
    fn a_program_runs_again_on_the_same_context() {
        let mut ctx = Context::new(1);
        let program = Program::compile("t = t + 1", &mut ctx).unwrap();
        let mut global = Megabuf::default();
        for _ in 0..3 {
            program.run(&mut ctx, &mut global);
        }
        assert_eq!(ctx.get("t"), 3.0);
        // A second program compiled later on the same context sees t.
        let other = Program::compile("t * 2", &mut ctx).unwrap();
        assert_eq!(other.run(&mut ctx, &mut global), 6.0);
    }
}
