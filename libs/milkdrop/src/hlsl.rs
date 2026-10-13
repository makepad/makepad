//! MilkDrop's shader dialect (HLSL shader-model 2/3 snippets) translated to
//! makepad shader source.
//!
//! A preset's `warp_N` / `comp_N` lines hold optional global declarations
//! and helper functions, then `shader_body { ... }`. The body reads MilkDrop's
//! inputs (`uv`, `uv_orig`, `rad`, `ang`, `hue_shader`), its uniforms (`time`,
//! `q1`..`q32`, `roam_cos`, `texsize`, ...) and its samplers
//! (`sampler_main`, `sampler_fw_noise_lq`, `sampler_blur1`, named image
//! textures), and writes `ret`.
//!
//! The translation is a small compiler: preprocess (`#define` objects,
//! `#if 0/1`), lex, parse to an AST, then ONE pass that type-checks and emits
//! makepad source, inserting what HLSL does implicitly (scalar splat, vector
//! truncation, int/bool to float) and rewriting what the makepad language
//! does not have (ternary, swizzle writes, bool-valued variables, `float2x2`,
//! small arrays, unbounded loops). The output is a set of methods on the
//! base draw shaders in [`crate::shaders`]: `milk_body` plus one method per
//! helper function the preset defines.
//!
//! Anything outside the supported subset is a [`TranslateError`]; the
//! renderer then runs the MilkDrop 1 default for that shader, which is what
//! MilkDrop itself does when a shader fails to compile.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// Which of the two preset shaders this is: they share the language but
/// differ in inputs (`hue_shader` exists only in the composite).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderKind {
    Warp,
    Comp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranslateError {
    pub message: String,
}

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn err<T>(message: impl Into<String>) -> Result<T, TranslateError> {
    Err(TranslateError { message: message.into() })
}

/// A texture the translated shader samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Texture {
    Main,
    Blur(u8),
    NoiseLq,
    NoiseLqLite,
    NoiseMq,
    NoiseHq,
    NoiseVolLq,
    NoiseVolHq,
    /// A named image texture (`sampler_worms`), by image slot.
    Image(u8),
}

/// How many named image textures one shader may sample.
pub const IMAGE_SLOTS: usize = 4;

/// The result of translating one shader.
#[derive(Clone, Debug, Default)]
pub struct Translation {
    /// Makepad object members (methods) to put inside a subclass of the
    /// base draw shader: `milk_body: fn(...) -> vec3 {...}` and helpers.
    pub members: String,
    /// Named image textures by slot (`worms`, `rand00`, ...), in slot order.
    pub images: Vec<String>,
    /// Textures the shader reads.
    pub textures: HashSet<Texture>,
}

// ---------------------------------------------------------------------------
// Preprocessor
// ---------------------------------------------------------------------------

/// Remove comments, apply `#if 0 / #if 1 / #else / #endif` and object-like
/// `#define`s. Function-like macros are not used by the corpus and fail.
fn preprocess(src: &str) -> Result<String, TranslateError> {
    let no_comments = strip_comments(src);
    let mut defines: Vec<(String, String)> = Vec::new();
    // Stack of (this branch active, any branch taken).
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let mut out = String::with_capacity(no_comments.len());
    for line in no_comments.lines() {
        let trimmed = line.trim_start();
        if let Some(directive) = trimmed.strip_prefix('#') {
            let directive = directive.trim_start();
            let (word, rest) = directive.split_once(|c: char| c.is_whitespace()).unwrap_or((directive, ""));
            let active = stack.iter().all(|(a, _)| *a);
            match word {
                "if" => {
                    let cond = rest.trim();
                    let value = match cond.parse::<i64>() {
                        Ok(v) => v != 0,
                        Err(_) => return err(format!("#if {cond}: only #if 0 / #if 1 are supported")),
                    };
                    stack.push((value, value));
                }
                "ifdef" | "ifndef" => {
                    let name = rest.trim();
                    let defined = defines.iter().any(|(n, _)| n == name);
                    let value = if word == "ifdef" { defined } else { !defined };
                    stack.push((value, value));
                }
                "else" => match stack.last_mut() {
                    Some((a, taken)) => {
                        *a = !*taken;
                        *taken = true;
                    }
                    None => return err("#else without #if"),
                },
                "endif" => {
                    if stack.pop().is_none() {
                        return err("#endif without #if");
                    }
                }
                "define" if active => {
                    let rest = rest.trim();
                    let (name, value) =
                        rest.split_once(|c: char| c.is_whitespace()).unwrap_or((rest, ""));
                    if name.contains('(') {
                        return err("function-like #define is not supported");
                    }
                    defines.retain(|(n, _)| n != name);
                    defines.push((name.to_string(), value.trim().to_string()));
                }
                "undef" if active => {
                    let name = rest.trim();
                    defines.retain(|(n, _)| n != name);
                }
                _ if !active => {}
                // `#pragma`, `#line` and friends: nothing to do.
                _ => {}
            }
            out.push('\n');
            continue;
        }
        if stack.iter().all(|(a, _)| *a) {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !stack.is_empty() {
        return err("#if without #endif");
    }
    if defines.is_empty() {
        return Ok(out);
    }
    // Replace defined names token by token, repeatedly (a define may name
    // another), bounded so a self-referencing define terminates.
    let mut text = out;
    for _ in 0..8 {
        let mut changed = false;
        let mut next = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c.is_ascii_alphabetic() || c == b'_' {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let word = &text[start..i];
                match defines.iter().rev().find(|(n, _)| n == word) {
                    Some((_, value)) => {
                        next.push_str(value);
                        changed = true;
                    }
                    None => next.push_str(word),
                }
                continue;
            }
            if c.is_ascii_digit() {
                // Numbers like `1e10` must not have their `e10` replaced.
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                    i += 1;
                }
                next.push_str(&text[start..i]);
                continue;
            }
            let ch = text[i..].chars().next().unwrap();
            next.push(ch);
            i += ch.len_utf8();
        }
        text = next;
        if !changed {
            break;
        }
    }
    Ok(text)
}

fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                if bytes[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            out.push(' ');
            continue;
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64, bool),
    Ident(String),
    Punct(&'static str),
    End,
}

const PUNCT: &[&str] = &[
    "<<=", ">>=", "++", "--", "+=", "-=", "*=", "/=", "%=", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "&=", "|=",
    "^=", "+", "-", "*", "/", "%", "<", ">", "=", "!", "~", "&", "|", "^", "?", ":", ";", ",", ".", "(", ")", "{", "}",
    "[", "]",
];

fn lex(src: &str) -> Result<Vec<Tok>, TranslateError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if c.is_ascii_digit() || (c == b'.' && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit())) {
            let mut is_float = false;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                if bytes[i] == b'.' {
                    is_float = true;
                }
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
                    is_float = true;
                } else {
                    i = save;
                }
            }
            let text = &src[start..i];
            // Suffixes: 1.0f, 2h, 3L.
            if i < bytes.len() && matches!(bytes[i], b'f' | b'F' | b'h' | b'H' | b'l' | b'L') {
                i += 1;
                is_float = true;
            }
            let mut t = text;
            while !t.is_empty() && t.parse::<f64>().is_err() {
                t = &t[..t.len() - 1];
            }
            out.push(Tok::Num(t.parse().unwrap_or(0.0), is_float));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(src[start..i].to_string()));
            continue;
        }
        let rest = &src[i..];
        match PUNCT.iter().find(|p| rest.starts_with(**p)) {
            Some(p) => {
                out.push(Tok::Punct(p));
                i += p.len();
            }
            None => {
                let ch = rest.chars().next().unwrap();
                return err(format!("unexpected character '{ch}'"));
            }
        }
    }
    out.push(Tok::End);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    /// Float vector of 1..4 components (1 = scalar). HLSL ints are floats
    /// here: the corpus uses them as loop counters and indices.
    F(u8),
    /// Bool of 1 component (vector bools do not occur in the corpus).
    Bool,
    /// 2x2 float matrix, stored row-major in a vec4.
    M2,
    /// 3x3 float matrix: a `mat3x3f` built from the HLSL elements in
    /// order, so each HLSL row is a column and `m[i]` is row `i`; HLSL's
    /// `mul(a, b)` is then `b * a`.
    M3,
    Void,
    Sampler,
}

impl Ty {
    fn comps(self) -> u8 {
        match self {
            Ty::F(n) => n,
            Ty::Bool => 1,
            Ty::M2 => 4,
            Ty::M3 => 9,
            _ => 0,
        }
    }
}

fn parse_type_name(name: &str) -> Option<Ty> {
    let (base, rest) = if let Some(r) = name.strip_prefix("float") {
        ("float", r)
    } else if let Some(r) = name.strip_prefix("half") {
        ("float", r)
    } else if let Some(r) = name.strip_prefix("double") {
        ("float", r)
    } else if let Some(r) = name.strip_prefix("int") {
        ("float", r)
    } else if let Some(r) = name.strip_prefix("uint") {
        ("float", r)
    } else if let Some(r) = name.strip_prefix("bool") {
        ("bool", r)
    } else if name == "void" {
        return Some(Ty::Void);
    } else if matches!(name, "sampler" | "sampler2D" | "sampler3D" | "texture") {
        return Some(Ty::Sampler);
    } else {
        return None;
    };
    match (base, rest) {
        ("bool", "") => Some(Ty::Bool),
        ("bool", _) => None,
        (_, "" | "1") => Some(Ty::F(1)),
        (_, "2") => Some(Ty::F(2)),
        (_, "3") => Some(Ty::F(3)),
        (_, "4") => Some(Ty::F(4)),
        (_, "2x2") => Some(Ty::M2),
        (_, "3x3") => Some(Ty::M3),
        (_, "1x1") => Some(Ty::F(1)),
        _ => None,
    }
}

fn mp_type(ty: Ty) -> &'static str {
    match ty {
        Ty::F(1) | Ty::Bool => "float",
        Ty::F(2) => "vec2",
        Ty::F(3) => "vec3",
        Ty::F(4) | Ty::M2 => "vec4",
        Ty::M3 => "mat3x3f",
        _ => "float",
    }
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Expr {
    Num(f64),
    Ident(String),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Assign(&'static str, Box<Expr>, Box<Expr>),
    /// `++x` / `x++` (the value is not used by the corpus, so both mean add).
    IncDec(&'static str, Box<Expr>),
    Call(String, Vec<Expr>),
    /// `(float3)x`
    Cast(Ty, Box<Expr>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// `{a, b, c}` array initialisers.
    List(Vec<Expr>),
}

#[derive(Clone, Debug)]
struct Declarator {
    name: String,
    array: Option<usize>,
    init: Option<Expr>,
}

#[derive(Clone, Debug)]
enum Stmt {
    Decl(Ty, Vec<Declarator>),
    Expr(Expr),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),
    Return(Option<Expr>),
    Break,
    Block(Vec<Stmt>),
    Empty,
}

#[derive(Clone, Debug)]
struct Param {
    ty: Ty,
    name: String,
    out: bool,
}

#[derive(Clone, Debug)]
struct Function {
    ret: Ty,
    name: String,
    params: Vec<Param>,
    body: Vec<Stmt>,
}

#[derive(Clone, Debug, Default)]
struct Program {
    globals: Vec<(Ty, Declarator)>,
    samplers: Vec<String>,
    functions: Vec<Function>,
    body: Vec<Stmt>,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }
    fn peek_at(&self, n: usize) -> &Tok {
        self.toks.get(self.pos + n).unwrap_or(&Tok::End)
    }
    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }
    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, p: &str) -> Result<(), TranslateError> {
        if self.eat(p) {
            Ok(())
        } else {
            err(format!("expected '{p}', found {:?}", self.peek()))
        }
    }
    fn ident(&mut self) -> Result<String, TranslateError> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.pos += 1;
                Ok(name)
            }
            t => err(format!("expected a name, found {t:?}")),
        }
    }
    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Ident(n) if n == w)
    }
    /// A type name at the cursor (after qualifiers).
    fn peek_type(&self) -> Option<Ty> {
        match self.peek() {
            Tok::Ident(n) => parse_type_name(n),
            _ => None,
        }
    }
    fn skip_qualifiers(&mut self) -> (bool, bool) {
        let mut out = false;
        let mut is_static = false;
        loop {
            match self.peek() {
                Tok::Ident(n) if matches!(n.as_str(), "static" | "const" | "uniform" | "in" | "inline" | "precise") => {
                    if n == "static" {
                        is_static = true;
                    }
                    self.pos += 1;
                }
                Tok::Ident(n) if matches!(n.as_str(), "out" | "inout") => {
                    out = true;
                    self.pos += 1;
                }
                _ => return (out, is_static),
            }
        }
    }

    fn program(&mut self) -> Result<Program, TranslateError> {
        let mut prog = Program::default();
        loop {
            if matches!(self.peek(), Tok::End) {
                return err("no shader_body");
            }
            if self.is_word("shader_body") {
                self.pos += 1;
                prog.body = self.block()?;
                return Ok(prog);
            }
            if self.eat(";") {
                continue;
            }
            self.skip_qualifiers();
            let type_name = self.ident()?;
            let Some(ty) = parse_type_name(&type_name) else {
                return err(format!("unknown type '{type_name}'"));
            };
            if ty == Ty::Sampler {
                // `sampler sampler_x;` / `sampler2D a, b;` — names only.
                loop {
                    let name = self.ident()?;
                    // `: register(s0)` annotations.
                    if self.eat(":") {
                        while !self.is(",") && !self.is(";") && !matches!(self.peek(), Tok::End) {
                            self.pos += 1;
                        }
                    }
                    // `= sampler_state { AddressU = WRAP; ... }`: the
                    // prefix naming convention carries the mode; skip it.
                    if self.eat("=") {
                        let mut depth = 0i32;
                        loop {
                            match self.peek() {
                                Tok::End => return err("unterminated sampler_state"),
                                Tok::Punct("{") => depth += 1,
                                Tok::Punct("}") => {
                                    depth -= 1;
                                    if depth <= 0 {
                                        self.pos += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            self.pos += 1;
                        }
                    }
                    prog.samplers.push(name);
                    if !self.eat(",") {
                        break;
                    }
                }
                self.expect(";")?;
                continue;
            }
            let name = self.ident()?;
            if self.eat("(") {
                let mut params = Vec::new();
                if !self.is(")") {
                    loop {
                        let (out, _) = self.skip_qualifiers();
                        let tn = self.ident()?;
                        let Some(pty) = parse_type_name(&tn) else {
                            return err(format!("unknown parameter type '{tn}'"));
                        };
                        let pname = self.ident()?;
                        if self.is("[") {
                            return err("array parameters are not supported");
                        }
                        params.push(Param { ty: pty, name: pname, out });
                        if !self.eat(",") {
                            break;
                        }
                    }
                }
                self.expect(")")?;
                if self.eat(";") {
                    continue; // forward declaration
                }
                let body = self.block()?;
                prog.functions.push(Function { ret: ty, name, params, body });
                continue;
            }
            // Global variables.
            let first = self.declarator_rest(name)?;
            prog.globals.push((ty, first));
            while self.eat(",") {
                let n = self.ident()?;
                let d = self.declarator_rest(n)?;
                prog.globals.push((ty, d));
            }
            self.expect(";")?;
        }
    }

    fn declarator_rest(&mut self, name: String) -> Result<Declarator, TranslateError> {
        let mut array = None;
        if self.eat("[") {
            match self.peek().clone() {
                Tok::Num(n, _) => {
                    self.pos += 1;
                    array = Some(n as usize);
                }
                _ => return err("array sizes must be numbers"),
            }
            self.expect("]")?;
        }
        let init = if self.eat("=") { Some(self.assign_expr()?) } else { None };
        Ok(Declarator { name, array, init })
    }

    fn block(&mut self) -> Result<Vec<Stmt>, TranslateError> {
        self.expect("{")?;
        let mut stmts = Vec::new();
        while !self.eat("}") {
            if matches!(self.peek(), Tok::End) {
                return err("missing '}'");
            }
            stmts.push(self.statement()?);
        }
        Ok(stmts)
    }

    fn statement(&mut self) -> Result<Stmt, TranslateError> {
        if self.is("{") {
            return Ok(Stmt::Block(self.block()?));
        }
        if self.eat(";") {
            return Ok(Stmt::Empty);
        }
        if self.is_word("if") {
            self.pos += 1;
            self.expect("(")?;
            let cond = self.expr()?;
            self.expect(")")?;
            let then = self.statement()?;
            let other = if self.is_word("else") {
                self.pos += 1;
                Some(Box::new(self.statement()?))
            } else {
                None
            };
            return Ok(Stmt::If(cond, Box::new(then), other));
        }
        if self.is_word("while") {
            self.pos += 1;
            self.expect("(")?;
            let cond = self.expr()?;
            self.expect(")")?;
            let body = self.statement()?;
            return Ok(Stmt::While(cond, Box::new(body)));
        }
        if self.is_word("do") {
            self.pos += 1;
            let body = self.statement()?;
            if !self.is_word("while") {
                return err("expected 'while' after do");
            }
            self.pos += 1;
            self.expect("(")?;
            let cond = self.expr()?;
            self.expect(")")?;
            self.eat(";");
            return Ok(Stmt::DoWhile(Box::new(body), cond));
        }
        if self.is_word("for") {
            self.pos += 1;
            self.expect("(")?;
            let init = if self.eat(";") {
                None
            } else {
                Some(Box::new(self.simple_statement()?))
            };
            let cond = if self.is(";") { None } else { Some(self.expr()?) };
            self.expect(";")?;
            let step = if self.is(")") { None } else { Some(self.expr()?) };
            self.expect(")")?;
            let body = self.statement()?;
            return Ok(Stmt::For(init, cond, step, Box::new(body)));
        }
        if self.is_word("return") {
            self.pos += 1;
            let value = if self.is(";") { None } else { Some(self.expr()?) };
            self.expect(";")?;
            return Ok(Stmt::Return(value));
        }
        if self.is_word("break") {
            self.pos += 1;
            self.expect(";")?;
            return Ok(Stmt::Break);
        }
        if self.is_word("continue") {
            return err("continue is not supported");
        }
        if self.is_word("discard") {
            return err("discard is not supported");
        }
        self.simple_statement()
    }

    /// A declaration or expression statement, including its `;`.
    fn simple_statement(&mut self) -> Result<Stmt, TranslateError> {
        let save = self.pos;
        self.skip_qualifiers();
        if let Some(ty) = self.peek_type() {
            // `float3(1,2,3).x;` is an expression, not a declaration.
            if matches!(self.peek_at(1), Tok::Ident(_)) {
                self.pos += 1;
                let mut decls = Vec::new();
                loop {
                    let name = self.ident()?;
                    decls.push(self.declarator_rest(name)?);
                    if !self.eat(",") {
                        break;
                    }
                }
                self.expect(";")?;
                return Ok(Stmt::Decl(ty, decls));
            }
        }
        self.pos = save;
        let e = self.expr()?;
        self.expect(";")?;
        Ok(Stmt::Expr(e))
    }

    /// Comma expressions are only used in `for` steps (`i++, j++`).
    fn expr(&mut self) -> Result<Expr, TranslateError> {
        let first = self.assign_expr()?;
        if self.is(",") {
            // Keep the list; the emitter runs each for its effect.
            let mut items = vec![first];
            while self.eat(",") {
                items.push(self.assign_expr()?);
            }
            return Ok(Expr::Call("__seq".into(), items));
        }
        Ok(first)
    }

    fn assign_expr(&mut self) -> Result<Expr, TranslateError> {
        if self.is("{") {
            self.pos += 1;
            let mut items = Vec::new();
            if !self.is("}") {
                loop {
                    items.push(self.assign_expr()?);
                    if !self.eat(",") || self.is("}") {
                        break;
                    }
                }
            }
            self.expect("}")?;
            return Ok(Expr::List(items));
        }
        let left = self.ternary()?;
        for op in ["=", "+=", "-=", "*=", "/=", "%="] {
            if self.eat(op) {
                let right = self.assign_expr()?;
                return Ok(Expr::Assign(op, Box::new(left), Box::new(right)));
            }
        }
        Ok(left)
    }

    fn ternary(&mut self) -> Result<Expr, TranslateError> {
        let cond = self.binary(0)?;
        if self.eat("?") {
            let a = self.assign_expr()?;
            self.expect(":")?;
            let b = self.assign_expr()?;
            return Ok(Expr::Ternary(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Ok(cond)
    }

    fn binary(&mut self, level: usize) -> Result<Expr, TranslateError> {
        const LEVELS: &[&[&str]] = &[
            &["||"],
            &["&&"],
            &["|"],
            &["^"],
            &["&"],
            &["==", "!="],
            &["<", ">", "<=", ">="],
            &["<<", ">>"],
            &["+", "-"],
            &["*", "/", "%"],
        ];
        if level >= LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        loop {
            let op = match self.peek() {
                Tok::Punct(p) if LEVELS[level].contains(p) => *p,
                _ => break,
            };
            self.pos += 1;
            let right = self.binary(level + 1)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, TranslateError> {
        for op in ["-", "+", "!", "~"] {
            if self.eat(op) {
                let inner = self.unary()?;
                return Ok(if op == "+" { inner } else { Expr::Unary(op, Box::new(inner)) });
            }
        }
        for op in ["++", "--"] {
            if self.eat(op) {
                let inner = self.unary()?;
                return Ok(Expr::IncDec(op, Box::new(inner)));
            }
        }
        // `(float3)x` casts.
        if self.is("(") {
            if let Tok::Ident(n) = self.peek_at(1) {
                if let Some(ty) = parse_type_name(n) {
                    if matches!(self.peek_at(2), Tok::Punct(")")) {
                        self.pos += 3;
                        let inner = self.unary()?;
                        return Ok(Expr::Cast(ty, Box::new(inner)));
                    }
                }
            }
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, TranslateError> {
        let mut e = self.primary()?;
        loop {
            if self.eat(".") {
                let member = self.ident()?;
                e = Expr::Member(Box::new(e), member);
            } else if self.eat("[") {
                let index = self.expr()?;
                self.expect("]")?;
                e = Expr::Index(Box::new(e), Box::new(index));
            } else if self.is("++") || self.is("--") {
                let op = if self.eat("++") { "++" } else { self.pos += 1; "--" };
                e = Expr::IncDec(op, Box::new(e));
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, TranslateError> {
        match self.peek().clone() {
            Tok::Num(v, _) => {
                self.pos += 1;
                Ok(Expr::Num(v))
            }
            Tok::Punct("(") => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Ident(name) => {
                self.pos += 1;
                if name == "true" {
                    return Ok(Expr::Num(1.0));
                }
                if name == "false" {
                    return Ok(Expr::Num(0.0));
                }
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.is(")") {
                        loop {
                            args.push(self.assign_expr()?);
                            if !self.eat(",") {
                                break;
                            }
                        }
                    }
                    self.expect(")")?;
                    return Ok(Expr::Call(name, args));
                }
                Ok(Expr::Ident(name))
            }
            t => err(format!("unexpected {t:?}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Emitter
// ---------------------------------------------------------------------------

/// A value during emission: makepad source text and its type.
#[derive(Clone, Debug)]
struct Val {
    code: String,
    ty: Ty,
}

#[derive(Clone, Debug)]
enum Local {
    Var(Ty, String),
    /// Scalarised array: element type and the per-element variable names.
    Array(Ty, Vec<String>),
}

struct FnSig {
    ret: Ty,
    params: Vec<Ty>,
    /// Globals the function reads, passed as trailing arguments.
    globals: Vec<String>,
}

/// Loop iteration cap: an unbounded GPU loop can hang the driver.
const GPU_LOOP_CAP: u32 = 256;

struct Emitter {
    kind: ShaderKind,
    out: String,
    indent: usize,
    /// Statements to run before the statement being emitted (hoisted
    /// ternaries, swizzle-write temporaries, bool materialisation).
    pre: Vec<String>,
    temp: usize,
    scopes: Vec<HashMap<String, Local>>,
    functions: HashMap<String, FnSig>,
    /// Global statics by HLSL name: type and emitted local name.
    globals: HashMap<String, Local>,
    samplers: HashSet<String>,
    images: Vec<String>,
    textures: HashSet<Texture>,
    /// The function being emitted (its return type), or None in the body.
    current_ret: Option<Ty>,
    /// Globals read by the function being emitted.
    read_globals: Vec<String>,
    /// Local names emitted in the current function, so a block-scoped
    /// redeclaration gets a fresh name (the makepad blocks we emit do not
    /// open scopes of their own).
    used_names: HashSet<String>,
}

fn fnum(v: f64) -> String {
    if !v.is_finite() {
        return "0.0".into();
    }
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    }
}

fn swizzle_index(c: char) -> Option<usize> {
    match c {
        'x' | 'r' | 's' => Some(0),
        'y' | 'g' | 't' => Some(1),
        'z' | 'b' | 'p' => Some(2),
        'w' | 'a' | 'q' => Some(3),
        _ => None,
    }
}

const XYZW: [char; 4] = ['x', 'y', 'z', 'w'];

impl Emitter {
    fn new(kind: ShaderKind) -> Self {
        Emitter {
            kind,
            out: String::new(),
            indent: 1,
            pre: Vec::new(),
            temp: 0,
            scopes: Vec::new(),
            functions: HashMap::new(),
            globals: HashMap::new(),
            samplers: HashSet::new(),
            images: Vec::new(),
            textures: HashSet::new(),
            current_ret: None,
            read_globals: Vec::new(),
            used_names: HashSet::new(),
        }
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn flush_pre(&mut self) {
        let pre = std::mem::take(&mut self.pre);
        for p in pre {
            self.line(&p);
        }
    }

    /// `base`, or `base_N` when a variable of that name was emitted already.
    fn unique(&mut self, base: String) -> String {
        if self.used_names.insert(base.clone()) {
            return base;
        }
        let mut k = 2;
        loop {
            let name = format!("{base}_{k}");
            if self.used_names.insert(name.clone()) {
                return name;
            }
            k += 1;
        }
    }

    fn fresh(&mut self) -> String {
        self.temp += 1;
        format!("t_{}", self.temp)
    }

    /// Materialise `v` into a temporary (so it can be read several times).
    fn hoist(&mut self, v: Val) -> Val {
        if is_simple(&v.code) {
            return v;
        }
        let name = self.fresh();
        self.pre.push(format!("let {name} = {}", v.code));
        Val { code: name, ty: v.ty }
    }

    fn lookup(&self, name: &str) -> Option<Local> {
        for scope in self.scopes.iter().rev() {
            if let Some(l) = scope.get(name) {
                return Some(l.clone());
            }
        }
        None
    }

    fn declare(&mut self, name: &str, local: Local) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), local);
    }

    // ---- conversions ----

    /// Convert `v` to `to`, the way HLSL converts implicitly.
    fn convert(&mut self, v: Val, to: Ty) -> Result<Val, TranslateError> {
        if v.ty == to {
            return Ok(v);
        }
        match (v.ty, to) {
            (Ty::Bool, Ty::F(n)) => {
                let f = self.bool_to_float(v);
                self.convert(f, Ty::F(n))
            }
            (Ty::F(_), Ty::Bool) => {
                let v = self.convert(v, Ty::F(1))?;
                Ok(Val { code: format!("({} != 0.0)", v.code), ty: Ty::Bool })
            }
            (Ty::F(1), Ty::F(n)) => {
                let v = self.hoist(v);
                let parts = vec![v.code.as_str(); n as usize].join(", ");
                Ok(Val { code: format!("{}({parts})", mp_type(Ty::F(n))), ty: Ty::F(n) })
            }
            (Ty::F(m), Ty::F(n)) if m > n => {
                let sw: String = XYZW[..n as usize].iter().collect();
                Ok(Val { code: format!("{}.{sw}", paren(&v.code)), ty: Ty::F(n) })
            }
            (Ty::F(1), Ty::M2) => {
                let v = self.hoist(v);
                Ok(Val { code: format!("vec4({0}, {0}, {0}, {0})", v.code), ty: Ty::M2 })
            }
            (Ty::F(4), Ty::M2) => Ok(Val { code: v.code, ty: Ty::M2 }),
            (Ty::F(1), Ty::M3) => {
                let v = self.hoist(v);
                Ok(Val { code: format!("mat3x3f({})", vec![v.code.as_str(); 9].join(", ")), ty: Ty::M3 })
            }
            (Ty::M2, Ty::F(4)) => Ok(Val { code: v.code, ty: Ty::F(4) }),
            (Ty::F(m), Ty::F(n)) => err(format!("cannot widen float{m} to float{n}")),
            (a, b) => err(format!("cannot convert {a:?} to {b:?}")),
        }
    }

    fn bool_to_float(&mut self, v: Val) -> Val {
        let name = self.fresh();
        self.pre.push(format!("var {name} = 0.0"));
        self.pre.push(format!("if {} {{ {name} = 1.0 }}", v.code));
        Val { code: name, ty: Ty::F(1) }
    }

    /// Bring two operands to a common type for a componentwise operation.
    fn unify(&mut self, a: Val, b: Val) -> Result<(Val, Val, Ty), TranslateError> {
        let ta = numeric(a.ty);
        let tb = numeric(b.ty);
        let a = self.convert(a, ta)?;
        let b = self.convert(b, tb)?;
        let (na, nb) = (ta.comps(), tb.comps());
        if ta == Ty::M2 || tb == Ty::M2 {
            let a = self.convert(a, Ty::M2)?;
            let b = self.convert(b, Ty::M2)?;
            return Ok((a, b, Ty::M2));
        }
        let n = if na == 1 {
            nb
        } else if nb == 1 {
            na
        } else {
            na.min(nb)
        };
        let t = Ty::F(n);
        let a = if na == 1 && n > 1 { a } else { self.convert(a, t)? };
        let b = if nb == 1 && n > 1 { b } else { self.convert(b, t)? };
        Ok((a, b, t))
    }

    // ---- expressions ----

    fn expr(&mut self, e: &Expr) -> Result<Val, TranslateError> {
        match e {
            Expr::Num(v) => Ok(Val { code: fnum(*v), ty: Ty::F(1) }),
            Expr::Ident(name) => self.ident(name),
            Expr::Unary(op, inner) => {
                let v = self.expr(inner)?;
                match *op {
                    "-" => {
                        let t = numeric(v.ty);
                        let v = self.convert(v, t)?;
                        Ok(Val { code: format!("(-{})", paren(&v.code)), ty: t })
                    }
                    "!" => {
                        let b = self.convert(v, Ty::Bool)?;
                        Ok(Val { code: format!("(!{})", paren(&b.code)), ty: Ty::Bool })
                    }
                    _ => err("bitwise not is not supported"),
                }
            }
            Expr::Binary(op, a, b) => self.binary(op, a, b),
            Expr::Ternary(c, a, b) => {
                let c = self.expr(c)?;
                let c = self.convert(c, Ty::Bool)?;
                // Branches are evaluated into temporaries; the corpus uses
                // side-effect-free branches, so hoisting both is safe.
                let av = self.expr(a)?;
                let bv = self.expr(b)?;
                let (av, bv, t) = self.unify(av, bv)?;
                let name = self.fresh();
                self.pre.push(format!("var {name} = {}", bv.code));
                self.pre.push(format!("if {} {{ {name} = {} }}", c.code, av.code));
                Ok(Val { code: name, ty: t })
            }
            Expr::Assign(op, target, value) => {
                self.assign(op, target, value)?;
                // `a = b = x`: the value is the target after the write.
                self.expr(target)
            }
            Expr::IncDec(op, target) => {
                let one = Expr::Num(1.0);
                let op = if *op == "++" { "+=" } else { "-=" };
                self.assign(op, target, &one)?;
                Ok(Val { code: "0.0".into(), ty: Ty::Void })
            }
            Expr::Cast(ty, inner) => {
                let v = self.expr(inner)?;
                // An explicit cast may truncate or splat.
                self.convert(v, *ty)
            }
            Expr::Member(base, member) => self.member(base, member),
            Expr::Index(base, index) => self.index(base, index),
            Expr::Call(name, args) => self.call(name, args),
            Expr::List(_) => err("an initializer list is only valid for arrays"),
        }
    }

    fn ident(&mut self, name: &str) -> Result<Val, TranslateError> {
        if let Some(local) = self.lookup(name) {
            return match local {
                Local::Var(ty, code) => Ok(Val { code, ty }),
                Local::Array(..) => err(format!("array '{name}' used as a value")),
            };
        }
        if let Some(global) = self.globals.get(name).cloned() {
            if self.current_ret.is_some() && !self.read_globals.iter().any(|g| g == name) {
                self.read_globals.push(name.to_string());
            }
            return match global {
                Local::Var(ty, code) => Ok(Val { code, ty }),
                Local::Array(..) => err(format!("array '{name}' used as a value")),
            };
        }
        if let Some((code, ty)) = self.builtin_value(name)? {
            return Ok(Val { code, ty });
        }
        err(format!("unknown name '{name}'"))
    }

    /// MilkDrop inputs, uniforms and constants.
    fn builtin_value(&mut self, name: &str) -> Result<Option<(String, Ty)>, TranslateError> {
        let q = |i: usize| format!("self.u_q{}.{}", i / 4, XYZW[i % 4]);
        let f1 = Ty::F(1);
        let v = match name {
            "time" => ("self.u_time.x".to_string(), f1),
            "fps" => ("self.u_time.y".into(), f1),
            "frame" => ("self.u_time.z".into(), f1),
            "progress" => ("self.u_time.w".into(), f1),
            "bass" => ("self.u_audio.x".into(), f1),
            "mid" => ("self.u_audio.y".into(), f1),
            "treb" => ("self.u_audio.z".into(), f1),
            "vol" => ("self.u_audio.w".into(), f1),
            "bass_att" => ("self.u_audio_att.x".into(), f1),
            "mid_att" => ("self.u_audio_att.y".into(), f1),
            "treb_att" => ("self.u_audio_att.z".into(), f1),
            "vol_att" => ("self.u_audio_att.w".into(), f1),
            "aspect" => ("self.u_aspect".into(), Ty::F(4)),
            "texsize" => ("self.texsize_main()".into(), Ty::F(4)),
            "rand_frame" => ("self.u_rand_frame".into(), Ty::F(4)),
            "rand_preset" => ("self.u_rand_preset".into(), Ty::F(4)),
            "roam_cos" => ("self.u_roam_cos".into(), Ty::F(4)),
            "roam_sin" => ("self.u_roam_sin".into(), Ty::F(4)),
            "slow_roam_cos" => ("self.u_slow_roam_cos".into(), Ty::F(4)),
            "slow_roam_sin" => ("self.u_slow_roam_sin".into(), Ty::F(4)),
            "blur1_min" => ("self.u_blur_min.x".into(), f1),
            "blur2_min" => ("self.u_blur_min.y".into(), f1),
            "blur3_min" => ("self.u_blur_min.z".into(), f1),
            "blur1_max" => ("self.u_blur_max.x".into(), f1),
            "blur2_max" => ("self.u_blur_max.y".into(), f1),
            "blur3_max" => ("self.u_blur_max.z".into(), f1),
            "M_PI" => (fnum(std::f64::consts::PI), f1),
            "M_PI_2" => (fnum(std::f64::consts::PI * 2.0), f1),
            "M_INV_PI_2" => (fnum(1.0 / (std::f64::consts::PI * 2.0)), f1),
            _ => {
                if let Some(n) = name.strip_prefix('q').and_then(|n| n.parse::<usize>().ok()) {
                    if (1..=32).contains(&n) {
                        return Ok(Some((q(n - 1), f1)));
                    }
                }
                if let Some(pack) = name.strip_prefix("_q") {
                    let mut chars = pack.chars();
                    if let (Some(c @ 'a'..='h'), None) = (chars.next(), chars.next()) {
                        let i = c as usize - 'a' as usize;
                        return Ok(Some((format!("self.u_q{i}"), Ty::F(4))));
                    }
                }
                if let Some(sampler) = name.strip_prefix("texsize_") {
                    let tex = self.sampler_texture(&format!("sampler_{sampler}"))?;
                    return Ok(Some((texsize_expr(tex), Ty::F(4))));
                }
                if name == "hue_shader" && self.kind == ShaderKind::Comp {
                    return Ok(Some(("hue_shader".into(), Ty::F(3))));
                }
                if name.starts_with("rot_") {
                    return err(format!("'{name}' (random rotation matrices) is not supported"));
                }
                return Ok(None);
            }
        };
        Ok(Some(v))
    }

    fn binary(&mut self, op: &str, a: &Expr, b: &Expr) -> Result<Val, TranslateError> {
        match op {
            "&&" | "||" => {
                let a = self.expr(a)?;
                let a = self.convert(a, Ty::Bool)?;
                let b = self.expr(b)?;
                let b = self.convert(b, Ty::Bool)?;
                return Ok(Val { code: format!("({} {op} {})", a.code, b.code), ty: Ty::Bool });
            }
            "<" | ">" | "<=" | ">=" | "==" | "!=" => {
                let a = self.expr(a)?;
                let b = self.expr(b)?;
                let (ta, tb) = (numeric(a.ty), numeric(b.ty));
                let a = self.convert(a, ta)?;
                let b = self.convert(b, tb)?;
                if a.ty.comps() == 1 && b.ty.comps() == 1 {
                    return Ok(Val { code: format!("({} {op} {})", a.code, b.code), ty: Ty::Bool });
                }
                // Componentwise: a vector of 0/1 (HLSL's bool vector, which
                // the corpus converts to float or reduces with all/any).
                let (a, b, t) = self.unify(a, b)?;
                let a = self.convert(a, t)?;
                let b = self.convert(b, t)?;
                let one = splat_const(t, 1.0);
                let eps = splat_const(t, 1e-6);
                let code = match op {
                    ">=" => format!("step({}, {})", b.code, a.code),
                    "<=" => format!("step({}, {})", a.code, b.code),
                    ">" => format!("({one} - step({}, {}))", a.code, b.code),
                    "<" => format!("({one} - step({}, {}))", b.code, a.code),
                    "==" => format!("({one} - step({eps}, abs({} - {})))", a.code, b.code),
                    _ => format!("step({eps}, abs({} - {}))", a.code, b.code),
                };
                return Ok(Val { code, ty: t });
            }
            "|" | "&" | "^" | "<<" | ">>" => return err(format!("bitwise '{op}' is not supported")),
            _ => {}
        }
        let av = self.expr(a)?;
        let bv = self.expr(b)?;
        if av.ty == Ty::M3 || bv.ty == Ty::M3 {
            return self.m3_binary(op, av, bv);
        }
        if av.ty == Ty::M2 || bv.ty == Ty::M2 {
            // Componentwise on matrices (`m * 0.5`).
            let (a, b, t) = self.unify(av, bv)?;
            let code = match op {
                "%" => format!("modf({}, {})", a.code, b.code),
                _ => format!("({} {op} {})", a.code, b.code),
            };
            return Ok(Val { code, ty: t });
        }
        let (a, b, t) = self.unify(av, bv)?;
        let code = match op {
            "%" => {
                // A scalar operand must be splatted for modf's overloads.
                let a = self.convert(a, t)?;
                let b = self.convert(b, t)?;
                format!("modf({}, {})", a.code, b.code)
            }
            _ => format!("({} {op} {})", a.code, b.code),
        };
        Ok(Val { code, ty: t })
    }

    fn member(&mut self, base: &Expr, member: &str) -> Result<Val, TranslateError> {
        let v = self.expr(base)?;
        let v = if v.ty == Ty::Bool { self.bool_to_float(v) } else { v };
        let n = match v.ty {
            Ty::F(n) => n,
            _ => return err(format!("'.{member}' on a {:?}", v.ty)),
        };
        let mut idx = Vec::new();
        for c in member.chars() {
            match swizzle_index(c) {
                Some(i) if (i as u8) < n.max(1) => idx.push(i),
                _ => return err(format!("bad swizzle '.{member}'")),
            }
        }
        if idx.is_empty() || idx.len() > 4 {
            return err(format!("bad swizzle '.{member}'"));
        }
        if n == 1 {
            // `x.xxx` on a scalar is a splat.
            return self.convert(v, Ty::F(idx.len() as u8));
        }
        let sw: String = idx.iter().map(|i| XYZW[*i]).collect();
        Ok(Val { code: format!("{}.{sw}", paren(&v.code)), ty: Ty::F(idx.len() as u8) })
    }

    fn index(&mut self, base: &Expr, index: &Expr) -> Result<Val, TranslateError> {
        // Arrays by name.
        if let Expr::Ident(name) = base {
            let local = self.lookup(name).or_else(|| self.globals.get(name).cloned());
            if let Some(Local::Array(ty, names)) = local {
                if self.current_ret.is_some() && self.globals.contains_key(name) {
                    return err("global arrays are not readable inside functions");
                }
                return self.array_read(ty, &names, index);
            }
        }
        let v = self.expr(base)?;
        let i = self.expr(index)?;
        let i = self.convert(i, Ty::F(1))?;
        match v.ty {
            Ty::F(n) if n > 1 => {
                if let Some(k) = const_index(&i.code) {
                    if k < n as usize {
                        return Ok(Val { code: format!("{}.{}", paren(&v.code), XYZW[k]), ty: Ty::F(1) });
                    }
                }
                let v = self.hoist(v);
                let parts: Vec<String> = (0..n as usize).map(|k| format!("{}.{}", v.code, XYZW[k])).collect();
                Ok(self.select_chain(&i, &parts, Ty::F(1)))
            }
            Ty::M2 => {
                // A matrix row.
                let v = self.hoist(v);
                let rows = [format!("{}.xy", v.code), format!("{}.zw", v.code)];
                if let Some(k) = const_index(&i.code) {
                    if k < 2 {
                        return Ok(Val { code: rows[k].clone(), ty: Ty::F(2) });
                    }
                }
                Ok(self.select_chain(&i, &rows, Ty::F(2)))
            }
            Ty::M3 => {
                let v = self.hoist(v);
                let rows: Vec<String> = (0..3).map(|k| format!("{}[{k}]", v.code)).collect();
                if let Some(k) = const_index(&i.code) {
                    if k < 3 {
                        return Ok(Val { code: rows[k].clone(), ty: Ty::F(3) });
                    }
                }
                Ok(self.select_chain(&i, &rows, Ty::F(3)))
            }
            _ => err("indexing a scalar"),
        }
    }

    /// `values[index]` for a runtime index: a chain of compares into a temp.
    fn select_chain(&mut self, index: &Val, values: &[String], ty: Ty) -> Val {
        let index = self.hoist(index.clone());
        let name = self.fresh();
        self.pre.push(format!("var {name} = {}", values[0]));
        for (k, value) in values.iter().enumerate().skip(1) {
            self.pre.push(format!("if abs({} - {}) < 0.5 {{ {name} = {value} }}", index.code, fnum(k as f64)));
        }
        Val { code: name, ty }
    }

    fn array_read(&mut self, ty: Ty, names: &[String], index: &Expr) -> Result<Val, TranslateError> {
        let i = self.expr(index)?;
        let i = self.convert(i, Ty::F(1))?;
        if let Some(k) = const_index(&i.code) {
            return match names.get(k) {
                Some(n) => Ok(Val { code: n.clone(), ty }),
                None => err("array index out of range"),
            };
        }
        Ok(self.select_chain(&i, names, ty))
    }

    // ---- assignment ----

    fn assign(&mut self, op: &str, target: &Expr, value: &Expr) -> Result<(), TranslateError> {
        let bin = match op {
            "=" => None,
            "+=" => Some("+"),
            "-=" => Some("-"),
            "*=" => Some("*"),
            "/=" => Some("/"),
            _ => Some("%"),
        };
        // The new value, computed from the current one for compound ops.
        let compute = |this: &mut Self, current: Val| -> Result<Val, TranslateError> {
            let v = this.expr(value)?;
            match bin {
                None => Ok(v),
                Some("%") => {
                    let (a, b, t) = this.unify(current, v)?;
                    let a = this.convert(a, t)?;
                    let b = this.convert(b, t)?;
                    Ok(Val { code: format!("modf({}, {})", a.code, b.code), ty: t })
                }
                Some(b) => {
                    let (x, y, t) = this.unify(current, v)?;
                    Ok(Val { code: format!("({} {b} {})", x.code, y.code), ty: t })
                }
            }
        };
        match target {
            Expr::Ident(name) => {
                let (ty, var) = self.assignable(name)?;
                let new = compute(self, Val { code: var.clone(), ty })?;
                let new = self.convert(new, ty)?;
                self.pre.push(format!("{var} = {}", new.code));
                Ok(())
            }
            Expr::Member(base, member) => {
                let Expr::Ident(name) = base.as_ref() else {
                    return err("assignment to a member of an expression");
                };
                let (ty, var) = self.assignable(name)?;
                let n = match ty {
                    Ty::F(n) if n > 1 => n,
                    Ty::F(1) => {
                        // `x.x = ...` on a scalar.
                        let new = compute(self, Val { code: var.clone(), ty })?;
                        let new = self.convert(new, Ty::F(1))?;
                        self.pre.push(format!("{var} = {}", new.code));
                        return Ok(());
                    }
                    _ => return err("member assignment on a non-vector"),
                };
                let mut idx = Vec::new();
                for c in member.chars() {
                    match swizzle_index(c) {
                        Some(i) if (i as u8) < n && !idx.contains(&i) => idx.push(i),
                        _ => return err(format!("bad write swizzle '.{member}'")),
                    }
                }
                let sw: String = idx.iter().map(|i| XYZW[*i]).collect();
                let current = Val { code: format!("{var}.{sw}"), ty: Ty::F(idx.len() as u8) };
                let new = compute(self, current)?;
                let new = self.convert(new, Ty::F(idx.len() as u8))?;
                let new = self.hoist(new);
                let mut parts = Vec::new();
                for k in 0..n as usize {
                    match idx.iter().position(|i| *i == k) {
                        Some(p) if idx.len() == 1 => {
                            let _ = p;
                            parts.push(new.code.clone())
                        }
                        Some(p) => parts.push(format!("{}.{}", new.code, XYZW[p])),
                        None => parts.push(format!("{var}.{}", XYZW[k])),
                    }
                }
                self.pre.push(format!("{var} = {}({})", mp_type(ty), parts.join(", ")));
                Ok(())
            }
            Expr::Index(base, index) => {
                let Expr::Ident(name) = base.as_ref() else {
                    return err("assignment to an indexed expression");
                };
                match self.lookup(name) {
                    Some(Local::Array(ty, names)) => {
                        let i = self.expr(index)?;
                        let i = self.convert(i, Ty::F(1))?;
                        let current = self.array_read_val(ty, &names, &i);
                        let new = compute(self, current)?;
                        let new = self.convert(new, ty)?;
                        let new = self.hoist(new);
                        match const_index(&i.code) {
                            Some(k) if k < names.len() => self.pre.push(format!("{} = {}", names[k], new.code)),
                            Some(_) => return err("array index out of range"),
                            None => {
                                let i = self.hoist(i);
                                for (k, n) in names.iter().enumerate() {
                                    self.pre.push(format!(
                                        "if abs({} - {}) < 0.5 {{ {n} = {} }}",
                                        i.code,
                                        fnum(k as f64),
                                        new.code
                                    ));
                                }
                            }
                        }
                        Ok(())
                    }
                    Some(Local::Var(Ty::F(n), _)) if n > 1 => {
                        let i = self.expr(index)?;
                        match const_index(&i.code) {
                            Some(k) if k < n as usize => {
                                let member = XYZW[k].to_string();
                                let target = Expr::Member(Box::new(Expr::Ident(name.clone())), member);
                                self.assign(op, &target, value)
                            }
                            _ => err("dynamic component writes are not supported"),
                        }
                    }
                    _ => err(format!("cannot index-assign '{name}'")),
                }
            }
            _ => err("unsupported assignment target"),
        }
    }

    fn array_read_val(&mut self, ty: Ty, names: &[String], i: &Val) -> Val {
        match const_index(&i.code) {
            Some(k) if k < names.len() => Val { code: names[k].clone(), ty },
            _ => self.select_chain(i, names, ty),
        }
    }

    /// A writable variable: locals, `ret`, `uv`. Globals are read-only inside
    /// functions (they are passed in by value).
    fn assignable(&mut self, name: &str) -> Result<(Ty, String), TranslateError> {
        match self.lookup(name) {
            Some(Local::Var(ty, code)) => return Ok((ty, code)),
            Some(Local::Array(..)) => return err("assigning a whole array"),
            None => {}
        }
        if let Some(Local::Var(ty, code)) = self.globals.get(name).cloned() {
            if self.current_ret.is_some() {
                return err(format!("function writes global '{name}'"));
            }
            return Ok((ty, code));
        }
        err(format!("cannot assign to '{name}'"))
    }

    // ---- calls ----

    fn args(&mut self, args: &[Expr]) -> Result<Vec<Val>, TranslateError> {
        args.iter().map(|a| self.expr(a)).collect()
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> Result<Val, TranslateError> {
        if name == "__seq" {
            // The comma operator: every item runs, the last is the value.
            let mut last = Val { code: "0.0".into(), ty: Ty::Void };
            for a in args {
                last = self.expr(a)?;
            }
            return Ok(last);
        }
        // Constructors.
        if let Some(ty) = parse_type_name(name) {
            return self.construct(ty, args);
        }
        // Texture reads, and `modf`'s out parameter.
        match name {
            "modf" => {
                // `modf(x, whole)` stores the integer part in `whole` and
                // is the fraction, both with x's sign.
                if args.len() != 2 {
                    return err("modf takes 2 arguments");
                }
                let whole = Expr::Call("trunc".into(), vec![args[0].clone()]);
                self.assign("=", &args[1], &whole)?;
                let x = self.expr(&args[0])?;
                let x = self.hoist(x);
                return Ok(Val { code: format!("({0} - trunc({0}))", x.code), ty: x.ty });
            }
            "tex2D" | "tex2d" | "tex2Dbias" | "tex2Dlod" => {
                if args.len() != 2 {
                    return err(format!("{name} takes 2 arguments"));
                }
                let sampler = sampler_name(&args[0])?;
                let uv = self.expr(&args[1])?;
                let uv = self.convert(uv, Ty::F(2))?;
                let code = self.sample2d(&sampler, &uv.code)?;
                return Ok(Val { code, ty: Ty::F(4) });
            }
            "tex3D" | "tex3d" => {
                if args.len() != 2 {
                    return err("tex3D takes 2 arguments");
                }
                let sampler = sampler_name(&args[0])?;
                let uvw = self.expr(&args[1])?;
                let uvw = self.convert(uvw, Ty::F(3))?;
                let tex = self.sampler_texture(&sampler)?;
                let method = match tex {
                    Texture::NoiseVolLq => "noisevol_lq",
                    Texture::NoiseVolHq => "noisevol_hq",
                    _ => return err(format!("tex3D on 2D sampler '{sampler}'")),
                };
                return Ok(Val { code: format!("self.{method}({})", uvw.code), ty: Ty::F(4) });
            }
            "GetPixel" | "GetMain" => {
                let uv = self.one_arg(name, args, Ty::F(2))?;
                self.textures.insert(Texture::Main);
                return Ok(Val { code: format!("self.get_main({})", uv.code), ty: Ty::F(3) });
            }
            "GetBlur1" | "GetBlur2" | "GetBlur3" => {
                let level = name.as_bytes()[7] - b'0';
                let uv = self.one_arg(name, args, Ty::F(2))?;
                self.textures.insert(Texture::Blur(level));
                return Ok(Val { code: format!("self.get_blur{level}({})", uv.code), ty: Ty::F(3) });
            }
            "lum" => {
                // MilkDrop's lum is `dot(x, float3(0.32,0.49,0.29))`: a
                // float2 argument truncates the weights, a scalar splats.
                if args.len() != 1 {
                    return err("lum takes 1 argument");
                }
                let v = self.expr(&args[0])?;
                let v = if v.ty == Ty::Bool { self.bool_to_float(v) } else { v };
                return Ok(match v.ty.comps() {
                    1 => Val { code: format!("({} * 1.1)", v.code), ty: Ty::F(1) },
                    2 => Val { code: format!("dot({}, vec2(0.32, 0.49))", v.code), ty: Ty::F(1) },
                    _ => {
                        let v = self.convert(v, Ty::F(3))?;
                        Val { code: format!("dot({}, vec3(0.32, 0.49, 0.29))", v.code), ty: Ty::F(1) }
                    }
                });
            }
            _ => {}
        }
        if self.functions.contains_key(name) {
            return self.user_call(name, args);
        }
        self.math(name, args)
    }

    fn one_arg(&mut self, name: &str, args: &[Expr], ty: Ty) -> Result<Val, TranslateError> {
        if args.len() != 1 {
            return err(format!("{name} takes 1 argument"));
        }
        let v = self.expr(&args[0])?;
        self.convert(v, ty)
    }

    fn construct(&mut self, ty: Ty, args: &[Expr]) -> Result<Val, TranslateError> {
        let vals = self.args(args)?;
        let want = ty.comps() as usize;
        if vals.len() == 1 {
            let v = vals.into_iter().next().unwrap();
            let v = if v.ty == Ty::Bool { self.bool_to_float(v) } else { v };
            if v.ty.comps() == 1 {
                return self.convert(v, ty);
            }
            if v.ty == ty {
                return Ok(v);
            }
            if ty == Ty::M3 || v.ty == Ty::M3 {
                return err(format!("{:?} from {:?}", ty, v.ty));
            }
            if v.ty.comps() as usize >= want {
                // `float3(v4)` truncates.
                let t = if ty == Ty::M2 { Ty::F(4) } else { ty };
                let c = self.convert(v, t)?;
                return Ok(Val { code: c.code, ty });
            }
            return err(format!("{:?} from {:?}", ty, v.ty));
        }
        let mut parts = Vec::new();
        let mut total = 0usize;
        for v in vals {
            let v = if v.ty == Ty::Bool { self.bool_to_float(v) } else { v };
            if ty == Ty::M3 {
                // `mat3x3f` takes its nine elements one by one.
                match v.ty {
                    Ty::F(1) => parts.push(v.code),
                    Ty::F(n) => {
                        let v = self.hoist(v);
                        parts.extend((0..n as usize).map(|k| format!("{}.{}", v.code, XYZW[k])));
                    }
                    _ => return err("float3x3 argument is not a scalar or vector"),
                }
                total = parts.len();
                continue;
            }
            match v.ty {
                Ty::F(n) => {
                    total += n as usize;
                    parts.push(v.code);
                }
                Ty::M2 => {
                    total += 4;
                    parts.push(v.code);
                }
                _ => return err("constructor argument is not a number"),
            }
        }
        if total != want {
            return err(format!("constructor needs {want} components, got {total}"));
        }
        Ok(Val { code: format!("{}({})", mp_type(ty), parts.join(", ")), ty })
    }

    fn user_call(&mut self, name: &str, args: &[Expr]) -> Result<Val, TranslateError> {
        let (params, ret, globals) = {
            let sig = &self.functions[name];
            (sig.params.clone(), sig.ret, sig.globals.clone())
        };
        if params.len() != args.len() {
            return err(format!("{name} takes {} arguments", params.len()));
        }
        let mut codes = Vec::new();
        for (a, ty) in args.iter().zip(params) {
            let v = self.expr(a)?;
            let v = self.convert(v, ty)?;
            codes.push(v.code);
        }
        for g in globals {
            let v = self.ident(&g)?;
            codes.push(v.code);
        }
        Ok(Val { code: format!("self.fn_{name}({})", codes.join(", ")), ty: ret })
    }

    fn math(&mut self, name: &str, args: &[Expr]) -> Result<Val, TranslateError> {
        let vals = self.args(args)?;
        let arity = |n: usize| -> Result<(), TranslateError> {
            if vals.len() == n {
                Ok(())
            } else {
                err(format!("{name} takes {n} arguments"))
            }
        };
        // Componentwise unary.
        let unary = match name {
            "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "exp" | "exp2" | "log" | "log2" | "sqrt" | "abs"
            | "sign" | "floor" | "ceil" | "round" | "trunc" | "normalize" => Some(name),
            "frac" => Some("fract"),
            "rsqrt" => Some("inverseSqrt"),
            "sinh" | "cosh" | "tanh" => Some(name),
            _ => None,
        };
        if let Some(f) = unary {
            arity(1)?;
            let v = vals.into_iter().next().unwrap();
            let t = numeric(v.ty);
            let v = self.convert(v, t)?;
            let code = match f {
                // normalize of a scalar is its sign.
                "normalize" if t.comps() == 1 => format!("sign({})", v.code),
                // Guard the domains MilkDrop's HLSL returned something for.
                "sqrt" => format!("sqrt(max({}, {}))", v.code, zero(t)),
                "log" | "log2" => format!("{f}(max({}, {}))", v.code, splat_const(t, 1e-20)),
                "asin" | "acos" => format!("{f}(clamp({}, {}, {}))", v.code, splat_const(t, -1.0), splat_const(t, 1.0)),
                "inverseSqrt" => format!("inverseSqrt(max({}, {}))", v.code, splat_const(t, 1e-20)),
                _ => format!("{f}({})", v.code),
            };
            let ty = if f == "normalize" { t } else { t };
            return Ok(Val { code, ty });
        }
        match name {
            "all" | "any" => {
                arity(1)?;
                let v = vals.into_iter().next().unwrap();
                let v = if v.ty == Ty::Bool { self.bool_to_float(v) } else { v };
                let n = numeric(v.ty).comps();
                let v = self.hoist(v);
                if n == 1 {
                    return Ok(Val { code: format!("({} != 0.0)", v.code), ty: Ty::Bool });
                }
                let parts: Vec<String> = (0..n as usize).map(|k| format!("abs({}.{})", v.code, XYZW[k])).collect();
                let f = if name == "all" { "min" } else { "max" };
                let mut code = parts[0].clone();
                for p in &parts[1..] {
                    code = format!("{f}({code}, {p})");
                }
                Ok(Val { code: format!("({code} > 0.0)"), ty: Ty::Bool })
            }
            "log10" => {
                arity(1)?;
                let v = vals.into_iter().next().unwrap();
                let t = numeric(v.ty);
                let v = self.convert(v, t)?;
                Ok(Val { code: format!("(log(max({}, {})) * 0.4342944819)", v.code, splat_const(t, 1e-20)), ty: t })
            }
            "saturate" => {
                arity(1)?;
                let v = vals.into_iter().next().unwrap();
                let t = numeric(v.ty);
                let v = self.convert(v, t)?;
                Ok(Val { code: format!("clamp({}, {}, {})", v.code, zero(t), splat_const(t, 1.0)), ty: t })
            }
            "length" => {
                arity(1)?;
                let v = vals.into_iter().next().unwrap();
                let t = numeric(v.ty);
                let v = self.convert(v, t)?;
                if t.comps() == 1 {
                    return Ok(Val { code: format!("abs({})", v.code), ty: Ty::F(1) });
                }
                Ok(Val { code: format!("length({})", v.code), ty: Ty::F(1) })
            }
            "dot" | "distance" => {
                arity(2)?;
                let mut it = vals.into_iter();
                let (a, b) = (it.next().unwrap(), it.next().unwrap());
                let (a, b, t) = self.unify(a, b)?;
                let a = self.convert(a, t)?;
                let b = self.convert(b, t)?;
                if t.comps() == 1 {
                    let code = if name == "dot" {
                        format!("({} * {})", a.code, b.code)
                    } else {
                        format!("abs({} - {})", a.code, b.code)
                    };
                    return Ok(Val { code, ty: Ty::F(1) });
                }
                Ok(Val { code: format!("{name}({}, {})", a.code, b.code), ty: Ty::F(1) })
            }
            "cross" => {
                arity(2)?;
                let mut it = vals.into_iter();
                let a = self.convert(it.next().unwrap(), Ty::F(3))?;
                let b = self.convert(it.next().unwrap(), Ty::F(3))?;
                Ok(Val { code: format!("cross({}, {})", a.code, b.code), ty: Ty::F(3) })
            }
            "pow" | "atan2" | "min" | "max" | "step" | "fmod" => {
                arity(2)?;
                let mut it = vals.into_iter();
                let (a, b) = (it.next().unwrap(), it.next().unwrap());
                let (a, b, t) = self.unify(a, b)?;
                let a = self.convert(a, t)?;
                let b = self.convert(b, t)?;
                let code = match name {
                    // HLSL pow of a negative base is NaN on the GPU; MilkDrop
                    // shaders rely on abs() semantics in practice.
                    "pow" => format!("pow(abs({}), {})", a.code, b.code),
                    "fmod" => format!("modf({}, {})", a.code, b.code),
                    _ => format!("{name}({}, {})", a.code, b.code),
                };
                Ok(Val { code, ty: t })
            }
            "lerp" | "clamp" | "smoothstep" => {
                arity(3)?;
                let mut it = vals.into_iter();
                let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
                // Vector arguments truncate to the smallest; scalars splat.
                let sizes: Vec<u8> = [a.ty, b.ty, c.ty].iter().map(|t| numeric(*t).comps()).filter(|n| *n > 1).collect();
                let n = sizes.iter().copied().min().unwrap_or(1);
                let t = Ty::F(n);
                let a = self.convert(a, t)?;
                let b = self.convert(b, t)?;
                let c = self.convert(c, t)?;
                let f = if name == "lerp" { "mix" } else { name };
                Ok(Val { code: format!("{f}({}, {}, {})", a.code, b.code, c.code), ty: t })
            }
            "mul" => {
                arity(2)?;
                let mut it = vals.into_iter();
                let (a, b) = (it.next().unwrap(), it.next().unwrap());
                self.mul(a, b)
            }
            "reflect" => {
                arity(2)?;
                let mut it = vals.into_iter();
                let (i, n) = (it.next().unwrap(), it.next().unwrap());
                let (i, n, t) = self.unify(i, n)?;
                let i = self.convert(i, t)?;
                let n = self.convert(n, t)?;
                let i = self.hoist(i);
                let n = self.hoist(n);
                Ok(Val { code: format!("({0} - 2.0 * dot({1}, {0}) * {1})", i.code, n.code), ty: t })
            }
            _ => err(format!("unknown function '{name}'")),
        }
    }

    /// Componentwise operators with a `float3x3` operand: matrix ± matrix,
    /// and matrix times or over a scalar (`*` of two matrices is
    /// componentwise in HLSL but a product in makepad, so it is refused).
    fn m3_binary(&mut self, op: &str, a: Val, b: Val) -> Result<Val, TranslateError> {
        let a = if a.ty == Ty::Bool { self.bool_to_float(a) } else { a };
        let b = if b.ty == Ty::Bool { self.bool_to_float(b) } else { b };
        match (op, a.ty, b.ty) {
            ("+" | "-", Ty::M3, _) | ("+" | "-", _, Ty::M3) => {
                let a = self.convert(a, Ty::M3)?;
                let b = self.convert(b, Ty::M3)?;
                Ok(Val { code: format!("({} {op} {})", a.code, b.code), ty: Ty::M3 })
            }
            ("*", Ty::M3, Ty::F(1)) | ("*", Ty::F(1), Ty::M3) => {
                Ok(Val { code: format!("({} * {})", a.code, b.code), ty: Ty::M3 })
            }
            ("/", Ty::M3, Ty::F(1)) => Ok(Val { code: format!("({} * (1.0 / {}))", a.code, b.code), ty: Ty::M3 }),
            _ => err(format!("'{op}' on {:?} and {:?}", a.ty, b.ty)),
        }
    }

    /// HLSL `mul`: row vector × matrix, matrix × column vector, matrix ×
    /// matrix, and scalar forms.
    fn mul(&mut self, a: Val, b: Val) -> Result<Val, TranslateError> {
        match (a.ty, b.ty) {
            (Ty::F(2), Ty::M2) => {
                let v = self.hoist(a);
                let m = self.hoist(b);
                Ok(Val {
                    code: format!(
                        "vec2({0}.x * {1}.x + {0}.y * {1}.z, {0}.x * {1}.y + {0}.y * {1}.w)",
                        v.code, m.code
                    ),
                    ty: Ty::F(2),
                })
            }
            (Ty::M2, Ty::F(2)) => {
                let m = self.hoist(a);
                let v = self.hoist(b);
                Ok(Val {
                    code: format!(
                        "vec2({1}.x * {0}.x + {1}.y * {0}.y, {1}.x * {0}.z + {1}.y * {0}.w)",
                        m.code, v.code
                    ),
                    ty: Ty::F(2),
                })
            }
            (Ty::F(3), Ty::M3) | (Ty::M3, Ty::M3) => Ok(Val { code: format!("({} * {})", b.code, a.code), ty: a.ty }),
            (Ty::M3, Ty::F(3)) => Ok(Val { code: format!("({} * {})", b.code, a.code), ty: Ty::F(3) }),
            (Ty::M3, _) | (_, Ty::M3) if a.ty.comps() != 1 && b.ty.comps() != 1 => {
                err(format!("mul of {:?} and {:?}", a.ty, b.ty))
            }
            (Ty::M2, Ty::M2) => {
                let a = self.hoist(a);
                let b = self.hoist(b);
                Ok(Val {
                    code: format!(
                        "vec4({0}.x * {1}.x + {0}.y * {1}.z, {0}.x * {1}.y + {0}.y * {1}.w, {0}.z * {1}.x + {0}.w * {1}.z, {0}.z * {1}.y + {0}.w * {1}.w)",
                        a.code, b.code
                    ),
                    ty: Ty::M2,
                })
            }
            _ => {
                // Scalar / vector forms multiply componentwise.
                let (a, b, t) = self.unify(a, b)?;
                Ok(Val { code: format!("({} * {})", a.code, b.code), ty: t })
            }
        }
    }

    // ---- samplers ----

    fn sampler_texture(&mut self, sampler: &str) -> Result<Texture, TranslateError> {
        let base = sampler.strip_prefix("sampler_").unwrap_or(sampler);
        let base = strip_mode(base).1;
        let tex = match base {
            "main" => Texture::Main,
            "blur1" => Texture::Blur(1),
            "blur2" => Texture::Blur(2),
            "blur3" => Texture::Blur(3),
            "noise_lq" => Texture::NoiseLq,
            "noise_lq_lite" => Texture::NoiseLqLite,
            "noise_mq" => Texture::NoiseMq,
            "noise_hq" => Texture::NoiseHq,
            "noisevol_lq" => Texture::NoiseVolLq,
            "noisevol_hq" => Texture::NoiseVolHq,
            name => {
                let slot = match self.images.iter().position(|n| n == name) {
                    Some(i) => i,
                    None => {
                        if self.images.len() >= IMAGE_SLOTS {
                            return err("more than 4 image textures");
                        }
                        self.images.push(name.to_string());
                        self.images.len() - 1
                    }
                };
                Texture::Image(slot as u8)
            }
        };
        self.textures.insert(tex);
        Ok(tex)
    }

    fn sample2d(&mut self, sampler: &str, uv: &str) -> Result<String, TranslateError> {
        let base = sampler.strip_prefix("sampler_").unwrap_or(sampler);
        let (mode, _) = strip_mode(base);
        let tex = self.sampler_texture(sampler)?;
        let field = match tex {
            Texture::Main => "tex_main".to_string(),
            Texture::Blur(l) => format!("tex_blur{l}"),
            Texture::NoiseLq => "tex_noise_lq".into(),
            Texture::NoiseLqLite => "tex_noise_lq_lite".into(),
            Texture::NoiseMq => "tex_noise_mq".into(),
            Texture::NoiseHq => "tex_noise_hq".into(),
            Texture::NoiseVolLq | Texture::NoiseVolHq => return err("tex2D on a volume sampler"),
            Texture::Image(i) => format!("tex_img{i}"),
        };
        // MilkDrop's defaults: bilinear + wrap, except the blur textures,
        // which clamp. A two-letter prefix overrides (f/p = filter,
        // w/c = wrap/clamp).
        let (point, wrap) = match mode {
            Some((p, w)) => (p, w),
            None => (false, !matches!(tex, Texture::Blur(_))),
        };
        Ok(match (point, wrap) {
            (false, true) => format!("self.{field}.sample_repeat({uv})"),
            (false, false) => format!("self.{field}.sample({uv})"),
            (true, true) => format!("self.{field}.sample_nearest(fract({uv}))"),
            (true, false) => format!("self.{field}.sample_nearest({uv})"),
        })
    }

    // ---- statements ----

    fn stmts(&mut self, stmts: &[Stmt]) -> Result<(), TranslateError> {
        for s in stmts {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn scoped(&mut self, s: &Stmt) -> Result<(), TranslateError> {
        self.scopes.push(HashMap::new());
        let r = match s {
            Stmt::Block(items) => self.stmts(items),
            other => self.stmt(other),
        };
        self.scopes.pop();
        r
    }

    fn stmt(&mut self, s: &Stmt) -> Result<(), TranslateError> {
        match s {
            Stmt::Empty => Ok(()),
            // Makepad shaders have no bare blocks; the scope is ours.
            Stmt::Block(items) => {
                self.scopes.push(HashMap::new());
                let r = self.stmts(items);
                self.scopes.pop();
                r
            }
            Stmt::Decl(ty, decls) => {
                for d in decls {
                    self.decl(*ty, d, false)?;
                }
                Ok(())
            }
            Stmt::Expr(e) => {
                self.expr(e)?;
                self.flush_pre();
                Ok(())
            }
            Stmt::If(cond, then, other) => {
                let c = self.expr(cond)?;
                let c = self.convert(c, Ty::Bool)?;
                self.flush_pre();
                self.line(&format!("if {} {{", c.code));
                self.indent += 1;
                self.scoped(then)?;
                self.indent -= 1;
                match other {
                    Some(other) => {
                        self.line("} else {");
                        self.indent += 1;
                        self.scoped(other)?;
                        self.indent -= 1;
                        self.line("}");
                    }
                    None => self.line("}"),
                }
                Ok(())
            }
            Stmt::While(cond, body) => self.emit_loop(None, Some(cond), None, body, false),
            Stmt::DoWhile(body, cond) => self.emit_loop(None, Some(cond), None, body, true),
            Stmt::For(init, cond, step, body) => {
                self.scopes.push(HashMap::new());
                if let Some(init) = init {
                    self.stmt(init)?;
                }
                let r = self.emit_loop(None, cond.as_ref(), step.as_ref(), body, false);
                self.scopes.pop();
                r
            }
            Stmt::Return(value) => {
                let Some(ret) = self.current_ret else {
                    // `return` in shader_body ends the body with `ret`.
                    self.line("return ret");
                    return Ok(());
                };
                match value {
                    Some(v) => {
                        let v = self.expr(v)?;
                        let v = self.convert(v, ret)?;
                        self.flush_pre();
                        self.line(&format!("return {}", v.code));
                    }
                    None => self.line("return"),
                }
                Ok(())
            }
            Stmt::Break => {
                self.line("break");
                Ok(())
            }
        }
    }

    fn emit_loop(
        &mut self,
        _init: Option<&Stmt>,
        cond: Option<&Expr>,
        step: Option<&Expr>,
        body: &Stmt,
        cond_after: bool,
    ) -> Result<(), TranslateError> {
        let counter = self.fresh();
        self.line(&format!("var {counter} = 0.0"));
        self.line("loop {");
        self.indent += 1;
        self.line(&format!("if {counter} >= {} {{ break }}", fnum(GPU_LOOP_CAP as f64)));
        self.line(&format!("{counter} = {counter} + 1.0"));
        if !cond_after {
            if let Some(cond) = cond {
                let c = self.expr(cond)?;
                let c = self.convert(c, Ty::Bool)?;
                self.flush_pre();
                self.line(&format!("if !({}) {{ break }}", c.code));
            }
        }
        self.scoped(body)?;
        if let Some(step) = step {
            self.expr(step)?;
            self.flush_pre();
        }
        if cond_after {
            if let Some(cond) = cond {
                let c = self.expr(cond)?;
                let c = self.convert(c, Ty::Bool)?;
                self.flush_pre();
                self.line(&format!("if !({}) {{ break }}", c.code));
            }
        }
        self.indent -= 1;
        self.line("}");
        Ok(())
    }

    /// Declare a local (or, with `global`, a global emitted as a body local).
    fn decl(&mut self, ty: Ty, d: &Declarator, global: bool) -> Result<(), TranslateError> {
        if ty == Ty::Void || ty == Ty::Sampler {
            return err("bad variable type");
        }
        let store_ty = if ty == Ty::Bool { Ty::F(1) } else { ty };
        let mangled = self.unique(format!("{}_{}", if global { "g" } else { "l" }, d.name));
        if let Some(len) = d.array {
            if len == 0 || len > 64 {
                return err("array too large");
            }
            let names: Vec<String> = (0..len).map(|k| format!("{mangled}_{k}")).collect();
            let inits: Vec<Val> = match &d.init {
                Some(Expr::List(items)) => {
                    let mut out = Vec::new();
                    for it in items {
                        let v = self.expr(it)?;
                        out.push(self.convert(v, store_ty)?);
                    }
                    out
                }
                Some(_) => return err("array initialised without a list"),
                None => Vec::new(),
            };
            for (k, n) in names.iter().enumerate() {
                let init = match inits.get(k) {
                    Some(v) => v.code.clone(),
                    None => zero(store_ty),
                };
                self.pre.push(format!("var {n} = {init}"));
            }
            self.flush_pre();
            let local = Local::Array(store_ty, names);
            if global {
                self.globals.insert(d.name.clone(), local);
            } else {
                self.declare(&d.name, local);
            }
            return Ok(());
        }
        let init = match &d.init {
            // `float2x2 m = { a, b, c, d };` initialises componentwise.
            Some(Expr::List(items)) => self.construct(store_ty, items)?.code,
            Some(e) => {
                let v = self.expr(e)?;
                self.convert(v, store_ty)?.code
            }
            None => zero(store_ty),
        };
        self.flush_pre();
        self.line(&format!("var {mangled} = {init}"));
        let local = Local::Var(store_ty, mangled);
        if global {
            self.globals.insert(d.name.clone(), local);
        } else {
            self.declare(&d.name, local);
        }
        Ok(())
    }
}

/// Root names of every assignment / increment target in `stmts`.
fn collect_assigned(stmts: &[Stmt], out: &mut HashSet<String>) {
    fn root(e: &Expr) -> Option<&str> {
        match e {
            Expr::Ident(n) => Some(n),
            Expr::Member(b, _) | Expr::Index(b, _) => root(b),
            _ => None,
        }
    }
    fn expr(e: &Expr, out: &mut HashSet<String>) {
        match e {
            Expr::Assign(_, t, v) => {
                if let Some(n) = root(t) {
                    out.insert(n.to_string());
                }
                expr(t, out);
                expr(v, out);
            }
            Expr::IncDec(_, t) => {
                if let Some(n) = root(t) {
                    out.insert(n.to_string());
                }
            }
            Expr::Unary(_, a) | Expr::Cast(_, a) | Expr::Member(a, _) => expr(a, out),
            Expr::Binary(_, a, b) | Expr::Index(a, b) => {
                expr(a, out);
                expr(b, out);
            }
            Expr::Ternary(a, b, c) => {
                expr(a, out);
                expr(b, out);
                expr(c, out);
            }
            Expr::Call(_, args) | Expr::List(args) => args.iter().for_each(|a| expr(a, out)),
            Expr::Num(_) | Expr::Ident(_) => {}
        }
    }
    fn stmt(s: &Stmt, out: &mut HashSet<String>) {
        match s {
            Stmt::Decl(_, ds) => ds.iter().filter_map(|d| d.init.as_ref()).for_each(|e| expr(e, out)),
            Stmt::Expr(e) => expr(e, out),
            Stmt::If(c, a, b) => {
                expr(c, out);
                stmt(a, out);
                if let Some(b) = b {
                    stmt(b, out);
                }
            }
            Stmt::While(c, b) | Stmt::DoWhile(b, c) => {
                expr(c, out);
                stmt(b, out);
            }
            Stmt::For(i, c, st, b) => {
                if let Some(i) = i {
                    stmt(i, out);
                }
                if let Some(c) = c {
                    expr(c, out);
                }
                if let Some(st) = st {
                    expr(st, out);
                }
                stmt(b, out);
            }
            Stmt::Return(Some(e)) => expr(e, out),
            Stmt::Block(items) => items.iter().for_each(|s| stmt(s, out)),
            Stmt::Return(None) | Stmt::Break | Stmt::Empty => {}
        }
    }
    stmts.iter().for_each(|s| stmt(s, out));
}

/// Names of the constant-folded literal index, if `code` is one.
fn const_index(code: &str) -> Option<usize> {
    let v: f64 = code.parse().ok()?;
    if v >= 0.0 && v.fract() == 0.0 {
        Some(v as usize)
    } else {
        None
    }
}

fn numeric(ty: Ty) -> Ty {
    match ty {
        Ty::Bool => Ty::F(1),
        t => t,
    }
}

fn zero(ty: Ty) -> String {
    splat_const(ty, 0.0)
}

fn splat_const(ty: Ty, v: f64) -> String {
    let n = fnum(v);
    match ty {
        Ty::F(1) | Ty::Bool => n,
        Ty::F(k) => format!("{}({})", mp_type(Ty::F(k)), vec![n; k as usize].join(", ")),
        Ty::M2 => format!("vec4({n}, {n}, {n}, {n})"),
        Ty::M3 => format!("mat3x3f({})", vec![n; 9].join(", ")),
        _ => n,
    }
}

fn is_simple(code: &str) -> bool {
    code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

fn paren(code: &str) -> String {
    if is_simple(code) || (code.starts_with('(') && balanced_outer(code)) {
        code.to_string()
    } else {
        format!("({code})")
    }
}

/// True when `code` is one parenthesised group from first to last char.
fn balanced_outer(code: &str) -> bool {
    let mut depth = 0i32;
    for (i, c) in code.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && i != code.len() - 1 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// `fw_noise_lq` → (Some((point=false, wrap=true)), "noise_lq").
fn strip_mode(base: &str) -> (Option<(bool, bool)>, &str) {
    for (prefix, mode) in [("fw_", (false, true)), ("fc_", (false, false)), ("pw_", (true, true)), ("pc_", (true, false))]
    {
        if let Some(rest) = base.strip_prefix(prefix) {
            return (Some(mode), rest);
        }
    }
    (None, base)
}

fn sampler_name(e: &Expr) -> Result<String, TranslateError> {
    match e {
        Expr::Ident(n) => Ok(n.clone()),
        _ => err("texture reads need a sampler name"),
    }
}

/// `texsize_X`: (width, height, 1/width, 1/height).
fn texsize_expr(tex: Texture) -> String {
    match tex {
        Texture::Main => "self.texsize_main()".into(),
        Texture::Blur(l) => format!("self.texsize_of_blur{l}()"),
        Texture::NoiseLq => "vec4(256.0, 256.0, 0.00390625, 0.00390625)".into(),
        Texture::NoiseLqLite => "vec4(32.0, 32.0, 0.03125, 0.03125)".into(),
        Texture::NoiseMq => "vec4(256.0, 256.0, 0.00390625, 0.00390625)".into(),
        Texture::NoiseHq => "vec4(256.0, 256.0, 0.00390625, 0.00390625)".into(),
        Texture::NoiseVolLq | Texture::NoiseVolHq => "vec4(32.0, 32.0, 0.03125, 0.03125)".into(),
        Texture::Image(i) => format!("self.texsize_of_img{i}()"),
    }
}

/// Translate one shader. `kind` selects the inputs the body may read.
pub fn translate(src: &str, kind: ShaderKind) -> Result<Translation, TranslateError> {
    let text = preprocess(src)?;
    let toks = lex(&text)?;
    let mut parser = Parser { toks, pos: 0 };
    let prog = parser.program()?;

    let mut em = Emitter::new(kind);
    em.samplers = prog.samplers.iter().cloned().collect();

    // Globals become locals at the top of the body; functions read them as
    // extra parameters. Emit their initialisers first into a prologue.
    let mut prologue = Emitter::new(kind);
    prologue.indent = 2;
    prologue.scopes.push(HashMap::new());
    for (ty, d) in &prog.globals {
        prologue.decl(*ty, d, true)?;
    }
    em.globals = prologue.globals.clone();
    em.images = prologue.images.clone();
    em.textures = prologue.textures.clone();
    em.temp = prologue.temp;

    // Functions: signatures first (bodies may call each other in order).
    let mut functions_src = String::new();
    for f in &prog.functions {
        if f.params.iter().any(|p| p.out) {
            return err(format!("function '{}' has out parameters", f.name));
        }
        if f.ret == Ty::Sampler || f.params.iter().any(|p| matches!(p.ty, Ty::Sampler | Ty::Void)) {
            return err(format!("function '{}' passes samplers", f.name));
        }
        em.out.clear();
        em.indent = 2;
        em.scopes = vec![HashMap::new()];
        // bool functions return float (0/1): the signature says float.
        em.current_ret = Some(numeric(f.ret));
        em.read_globals.clear();
        em.used_names.clear();
        let mut params_src = Vec::new();
        for p in &f.params {
            let ty = if p.ty == Ty::Bool { Ty::F(1) } else { p.ty };
            let name = format!("p_{}", p.name);
            // Parameters are immutable in makepad; HLSL may assign them.
            params_src.push(format!("a_{}: {}", p.name, mp_type(ty)));
            em.line(&format!("var {name} = a_{}", p.name));
            em.used_names.insert(name.clone());
            em.declare(&p.name, Local::Var(ty, name));
        }
        em.stmts(&f.body)?;
        if !em.pre.is_empty() {
            em.flush_pre();
        }
        // Globals read by this function become trailing parameters. They
        // are bound inside the body under their global names already.
        let globals = em.read_globals.clone();
        for g in &globals {
            if let Some(Local::Var(ty, code)) = em.globals.get(g) {
                params_src.push(format!("{code}: {}", mp_type(*ty)));
            } else {
                return err(format!("function '{}' reads global array '{g}'", f.name));
            }
        }
        let ret = if f.ret == Ty::Void { String::new() } else { format!(" -> {}", mp_type(numeric(f.ret))) };
        let _ = writeln!(functions_src, "    fn_{}: fn({}){ret} {{", f.name, params_src.join(", "));
        functions_src.push_str(&em.out);
        functions_src.push_str("    }\n");
        em.functions.insert(
            f.name.clone(),
            FnSig { ret: numeric(f.ret), params: f.params.iter().map(|p| numeric(p.ty)).collect(), globals },
        );
    }

    // The body.
    em.out.clear();
    em.indent = 2;
    em.current_ret = None;
    em.scopes = vec![HashMap::new()];
    em.used_names.clear();
    for (name, ty) in [("uv", Ty::F(2)), ("uv_orig", Ty::F(2)), ("rad", Ty::F(1)), ("ang", Ty::F(1)), ("ret", Ty::F(3))] {
        em.used_names.insert(name.to_string());
        em.declare(name, Local::Var(ty, name.into()));
    }
    if kind == ShaderKind::Comp {
        em.used_names.insert("hue_shader".into());
        em.declare("hue_shader", Local::Var(Ty::F(3), "hue_shader".into()));
    }
    // HLSL lets a shader write its uniforms (as private copies): shadow the
    // ones this body assigns, initialised from the uniform.
    let mut assigned = HashSet::new();
    collect_assigned(&prog.body, &mut assigned);
    let mut assigned: Vec<String> = assigned.into_iter().collect();
    assigned.sort();
    for name in assigned {
        if em.lookup(&name).is_some() || em.globals.contains_key(&name) {
            continue;
        }
        if let Some((code, ty)) = em.builtin_value(&name)? {
            let local = em.unique(format!("s_{name}"));
            em.line(&format!("var {local} = {code}"));
            em.declare(&name, Local::Var(ty, local));
        }
    }
    em.stmts(&prog.body)?;
    em.flush_pre();

    let sig = match kind {
        ShaderKind::Warp => "uv_in: vec2, uv_orig_in: vec2, rad_in: float, ang_in: float",
        ShaderKind::Comp => "uv_in: vec2, uv_orig_in: vec2, rad_in: float, ang_in: float, hue_in: vec3",
    };
    let mut members = functions_src;
    let _ = writeln!(members, "    milk_body: fn({sig}) -> vec3 {{");
    members.push_str("        var uv = uv_in\n        var uv_orig = uv_orig_in\n        var rad = rad_in\n        var ang = ang_in\n");
    if kind == ShaderKind::Comp {
        members.push_str("        var hue_shader = hue_in\n");
    }
    members.push_str("        var ret = vec3(0.0, 0.0, 0.0)\n");
    members.push_str(&prologue.out);
    members.push_str(&em.out);
    members.push_str("        return ret\n    }\n");

    Ok(Translation { members, images: em.images, textures: em.textures })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(src: &str) -> String {
        translate(src, ShaderKind::Warp).unwrap_or_else(|e| panic!("{e}: {src}")).members
    }

    #[test]
    fn the_default_warp_translates() {
        let m = body("shader_body { ret = tex2D(sampler_main, uv).xyz; ret *= 0.97; }");
        assert!(m.contains("ret = (self.tex_main.sample_repeat(uv)).xyz"), "{m}");
        assert!(m.contains("ret = (ret * 0.97)"), "{m}");
    }

    #[test]
    fn implicit_truncation_and_splat() {
        let m = body("shader_body { float3 c = tex2D(sampler_fc_main, uv); float2 d = 0.5; ret = c + d.x; }");
        assert!(m.contains("var l_c = (self.tex_main.sample(uv)).xyz"), "{m}");
        assert!(m.contains("var l_d = vec2(0.5, 0.5)"), "{m}");
    }

    #[test]
    fn swizzle_writes_rebuild_the_vector() {
        let m = body("shader_body { ret = 0; ret.y += 1; ret.xz = float2(2,3); }");
        assert!(m.contains("let t_1 = (ret.y + 1.0)\n        ret = vec3(ret.x, t_1, ret.z)"), "{m}");
        assert!(m.contains("let t_2 = vec2(2.0, 3.0)\n        ret = vec3(t_2.x, ret.y, t_2.y)"), "{m}");
    }

    #[test]
    fn braceless_if_and_ternary() {
        let m = body("shader_body { float a = rad > 0.5 ? 1 : 0; if (a > 0) ret = 1; else ret = 0; }");
        assert!(m.contains("if (rad > 0.5) { t_1 = 1.0 }"), "{m}");
        assert!(m.contains("if (l_a > 0.0) {"), "{m}");
    }

    #[test]
    fn defines_and_if_blocks() {
        let m = body("#define MYSAMP sampler_noise_hq\n#if 0\nthis is garbage\n#else\n#endif\nshader_body { ret = tex2D(MYSAMP, uv).xyz; }");
        assert!(m.contains("self.tex_noise_hq.sample_repeat(uv)).xyz"), "{m}");
    }

    #[test]
    fn functions_and_globals() {
        let t = translate(
            "static float3 base = float3(q1, q2, 0); float2 rot(float2 p, float a) { return float2(p.x*cos(a) - p.y*sin(a), p.x*sin(a) + p.y*cos(a)) + base.xy; } shader_body { ret = GetPixel(rot(uv, time)); }",
            ShaderKind::Warp,
        )
        .unwrap();
        assert!(t.members.contains("fn_rot: fn(a_p: vec2, a_a: float, g_base: vec3) -> vec2 {"), "{}", t.members);
        assert!(t.members.contains("self.fn_rot(uv, self.u_time.x, g_base)"), "{}", t.members);
        assert!(t.members.contains("var g_base = vec3(self.u_q0.x, self.u_q0.y, 0.0)"), "{}", t.members);
    }

    #[test]
    fn loops_are_capped() {
        let m = body("shader_body { float s = 0; for (int i = 0; i < 8; i++) { s += i; } ret = s; }");
        assert!(m.contains("loop {"), "{m}");
        assert!(m.contains(">= 256.0 { break }"), "{m}");
        assert!(m.contains("if !((l_i < 8.0)) { break }"), "{m}");
    }

    #[test]
    fn matrices_and_mul() {
        let m = body("shader_body { float2x2 r = float2x2(cos(time), -sin(time), sin(time), cos(time)); ret = GetPixel(mul(uv - 0.5, r) + 0.5); }");
        assert!(m.contains("var l_r = vec4("), "{m}");
        assert!(m.contains(".x * l_r.x +"), "{m}");
    }

    #[test]
    fn modf_writes_the_whole_part() {
        let m = body("shader_body { float fb; float f = modf(uv.x * 255.0, fb); ret = float3(f, fb / 255.0, 0); }");
        assert!(m.contains("l_fb = trunc("), "{m}");
        assert!(m.contains("- trunc("), "{m}");
    }

    #[test]
    fn three_by_three_matrices() {
        let m = body("shader_body { float3x3 r = float3x3(1, 0, 0, 0, cos(time), -sin(time), float3(0, sin(time), cos(time))); float3 p = mul(float3(uv, 1), r); p = mul(r, p) * 0.5; float3x3 q = mul(r, r) + r * 2; ret = p + q[1]; }");
        assert!(m.contains("var l_r = mat3x3f(1.0, 0.0, 0.0, 0.0, cos("), "{m}");
        assert!(m.contains("(l_r * vec3("), "row vector times matrix: {m}");
        assert!(m.contains("(l_p * l_r)"), "matrix times column vector: {m}");
        assert!(m.contains("[1]"), "{m}");
        assert!(translate("shader_body { float3x3 a = 1; float3x3 b = a * a; ret = b[0]; }", ShaderKind::Comp).is_err());
    }

    #[test]
    fn samplers_images_and_texsize() {
        let t = translate(
            "sampler sampler_worms; shader_body { ret = tex2D(sampler_worms, uv * texsize_worms.xy).xyz + tex2D(sampler_pw_noise_lq, uv).xyz + tex3D(sampler_noisevol_hq, float3(uv, time)).xyz + GetBlur2(uv); }",
            ShaderKind::Comp,
        )
        .unwrap();
        assert_eq!(t.images, vec!["worms".to_string()]);
        assert!(t.members.contains("self.tex_img0.sample_repeat("), "{}", t.members);
        assert!(t.members.contains("self.texsize_of_img0()"), "{}", t.members);
        assert!(t.members.contains("self.tex_noise_lq.sample_nearest(fract(uv))"), "{}", t.members);
        assert!(t.members.contains("self.noisevol_hq(vec3(uv, self.u_time.x))"), "{}", t.members);
        assert!(t.textures.contains(&Texture::Blur(2)));
    }

    #[test]
    fn arrays_scalarise() {
        let m = body("shader_body { float3 m[3]; m[0] = GetPixel(uv); m[1] = 1; int k = 1; ret = m[k] + m[0]; }");
        assert!(m.contains("var l_m_0 = vec3(0.0, 0.0, 0.0)"), "{m}");
        assert!(m.contains("let t_1 = self.get_main(uv)\n        l_m_0 = t_1"), "{m}");
        assert!(m.contains("if abs(l_k - 1.0) < 0.5 { t_"), "{m}");
    }

    /// Print up to 3 corpus shaders failing with `MILKDROP_CAUSE`, from the
    /// preset directory `MILKDROP_CORPUS`. A development aid; does nothing
    /// without the variables.
    #[test]
    fn dump_failures_by_cause() {
        let (Some(dir), Ok(cause)) = (std::env::var_os("MILKDROP_CORPUS"), std::env::var("MILKDROP_CAUSE")) else {
            return;
        };
        let mut n = 0;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if !p.extension().is_some_and(|x| x == "milk") {
                    continue;
                }
                let preset = crate::Preset::parse(&std::fs::read(&p).unwrap());
                for (src, kind) in [(&preset.warp_shader, ShaderKind::Warp), (&preset.comp_shader, ShaderKind::Comp)] {
                    let Some(src) = src else { continue };
                    if let Err(e) = translate(src, kind) {
                        if e.message.contains(&cause) && n < 3 {
                            n += 1;
                            println!("=== {}\n{}\n--- {}", p.display(), src, e.message);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unsupported_things_fail_cleanly() {
        assert!(translate("shader_body { ret = mul(uv, rot_s1).xyz; }", ShaderKind::Warp).is_err());
        assert!(translate("shader_body { ret = 1 ", ShaderKind::Warp).is_err());
        assert!(translate("nothing here", ShaderKind::Warp).is_err());
        assert!(translate("void f(out float a) { a = 1; } shader_body { }", ShaderKind::Warp).is_err());
    }
}
