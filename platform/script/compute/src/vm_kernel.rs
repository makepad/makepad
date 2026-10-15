//! Kernels made from Splash values: a host hands the compiler the
//! declarations it read from a document (typed, [`Decl`]) and the entry as
//! the VM's own fn object, and the compiler finds everything the entry
//! reaches through the VM: each name the fn uses is looked up in the scope
//! the fn was made in, the way the VM would at runtime. A script fn found
//! there is compiled in (with what it reaches in turn), a number, colour or
//! float vector becomes a constant. No host reads or cuts document text,
//! and none is made: the compiler builds its syntax tree from the
//! declarations' values and from each fn's tokens as the VM read them.
//!
//! ```ignore
//! // the document:  let R = 3.5
//! //                let spin = fn(p, t) { return rot2(p, t * R) }
//! //                let sparks = Kernel{ count: 600 out: output(Sprite)
//! //                                     element: fn(i) { out[i].pos = spin(..) } }
//! let k = vm_kernel::compile(vm, &VmKernel {
//!     decls: vec![Decl::Output { name: "out".into(), ty: "Sprite".into(), stride: None, offset: None, buffer: None }],
//!     entry: Entry::Element,
//!     entry_fn: element_fn_object,
//!     math: MathMode::Fast,
//!     uses: vec![],
//!     bind: vec![],
//! }, &layouts, Backend::Native, &[])?;
//! ```
//!
//! Errors carry the document location of the code they point into
//! ([`KernelProgram::locate`]).

use crate::kernel::{self, KernelRef, Layout, MathMode};
use crate::parse::{Expr, ExprKind, FnDecl, Item, Parser, Span, Stmt, Tk, Token, TypeAnn};
use crate::{Backend, ShaderError};
use makepad_script::*;
use std::collections::HashSet;


/// One top-level declaration of a kernel (the kernel language's
/// `let name = input(..)`, `output(..)`, `emit_buffer(..)`, `param(..)`).
/// `ty` is an element type (f32, i32, vec2, vec3, vec4, mat4) or the name of
/// a host layout.
#[derive(Debug)]
pub enum Decl {
    Input { name: String, ty: String, stride: Option<u32>, offset: Option<u32>, buffer: Option<String> },
    Output { name: String, ty: String, stride: Option<u32>, offset: Option<u32>, buffer: Option<String> },
    /// Records of `record` (a type, or a width in words when `width` is set)
    /// emitted per element, at most `capacity` each.
    EmitBuffer { name: String, record: Record, capacity: u32, buffer: Option<String> },
    Param { name: String, default: f32, range: Option<(f32, f32)> },
    /// A record type: its fields and their zero values, in order
    /// (`struct Name {a: 0.0, b: vec3(0.0)}`).
    Struct { name: String, fields: Vec<(String, Const)> },
    /// A constant table of an element type (`let Name: i32 = [..]`).
    Table { name: String, ty: String, values: Vec<Const> },
}

#[derive(Debug)]
pub enum Record {
    Type(String),
    Words(u32),
}

impl Decl {
    #[inline(always)]
    pub fn name(&self) -> &str {
        match self {
            Decl::Input { name, .. } | Decl::Output { name, .. } | Decl::EmitBuffer { name, .. } | Decl::Param { name, .. } | Decl::Struct { name, .. } | Decl::Table { name, .. } => name,
        }
    }
}

/// The entry kinds (`fn vertex(i)` …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Entry {
    Element,
    Vertex,
    Instance,
    Primitive,
    ReduceSum,
    ReduceMin,
    ReduceMax,
}

impl Entry {
    pub fn name(self) -> &'static str {
        match self {
            Entry::Element => "element",
            Entry::Vertex => "vertex",
            Entry::Instance => "instance",
            Entry::Primitive => "primitive",
            Entry::ReduceSum => "reduce_sum",
            Entry::ReduceMin => "reduce_min",
            Entry::ReduceMax => "reduce_max",
        }
    }

    pub fn from_name(name: &str) -> Option<Entry> {
        Some(match name {
            "element" => Entry::Element,
            "vertex" => Entry::Vertex,
            "instance" => Entry::Instance,
            "primitive" => Entry::Primitive,
            "reduce_sum" => Entry::ReduceSum,
            "reduce_min" => Entry::ReduceMin,
            "reduce_max" => Entry::ReduceMax,
            _ => return None,
        })
    }
}

pub struct VmKernel {
    pub decls: Vec<Decl>,
    pub entry: Entry,
    /// The entry: a script fn of one parameter (the element index).
    pub entry_fn: ScriptObject,
    pub math: MathMode,
    /// Compiler modules (passed to [`compile`]) the program imports
    /// unqualified (`use path.*`).
    pub uses: Vec<String>,
    /// Script fns the host binds by name: compiled in under that name, for
    /// an entry (a host's own) that calls a fn the document supplies.
    pub bind: Vec<(String, ScriptObject)>,
}

/// A kernel's program as the compiler reads it: the syntax tree built from
/// the kernel's values (declarations, the constants and fns its entry
/// reaches, each fn from the VM's own tokens), and where each fn came from.
/// It holds no VM references, so it can move to a compiler thread.
pub(crate) struct KernelProgram {
    items: Vec<Item>,
    /// One past the last span of `items`.
    end: usize,
    /// Per compiled-in fn, its file and per token (span start, span end,
    /// 1-based line, column): document places for the compiler's errors.
    places: Vec<(String, Vec<(usize, usize, u32, u32)>)>,
    /// A fn's tokens that do not parse (start, end, message): reported, at
    /// its place, when the program compiles.
    error: Option<(usize, usize, String)>,
    /// The names the compiled-in fns use that none of them binds itself
    /// (a parameter, a `let`, `var` or loop variable), in first-use order:
    /// the declarations, the kernel's own inputs (`time`, …), and names
    /// nothing declares (a host may declare those it provides, such as
    /// signals, and build again).
    pub(crate) free: Vec<String>,
    /// The document's names compiled in as constants (their values as the
    /// document evaluated them).
    pub(crate) constants: Vec<String>,
}

impl KernelProgram {
    /// The document location (file, 1-based line and column) of a span
    /// position, when it falls inside a fn taken from the document.
    fn locate(&self, at: usize) -> Option<(String, u32, u32)> {
        for (file, toks) in &self.places {
            let (Some(first), Some(last)) = (toks.first(), toks.last()) else { continue };
            if at < first.0 || at > last.1 {
                continue;
            }
            let k = toks.partition_point(|t| t.0 <= at).max(1) - 1;
            let (start, end, line, col) = toks[k];
            let col = if at < end { col + (at - start) as u32 } else { col };
            return Some((file.clone(), line, col));
        }
        None
    }

    /// `message (file:line:col)` for an error, when its place is known.
    fn describe(&self, e: &ShaderError) -> String {
        match self.locate(e.start) {
            Some((file, line, col)) => format!("{} ({file}:{line}:{col})", e.message),
            None => e.message.clone(),
        }
    }

    /// Compiles the program; the errors' messages name the document place.
    pub(crate) fn compile(&self, layouts: &[Layout], backend: Backend, modules: &[crate::module::Module]) -> Result<KernelRef, Vec<ShaderError>> {
        self.compile_placed(layouts, backend, modules).map_err(|errors| errors.into_iter().map(|(e, _)| ShaderError { message: self.describe(&e), ..e }).collect())
    }

    /// Compiles the program; each error with its document place (file,
    /// 1-based line and column) when it points into a fn of the document.
    /// A program compiled before in this process (the same syntax tree,
    /// layouts, backend and modules: a document loaded again) is that
    /// kernel: native compiles of a large document take seconds.
    pub(crate) fn compile_placed(&self, layouts: &[Layout], backend: Backend, modules: &[crate::module::Module]) -> Result<KernelRef, Vec<(ShaderError, Option<(String, u32, u32)>)>> {
        if let Some((start, end, message)) = &self.error {
            return Err(vec![(ShaderError::new(*start, *end, message.clone()), self.locate(*start))]);
        }
        let key = self.key(layouts, backend, modules);
        if let Some(kernel) = COMPILED.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(kernel.clone());
        }
        match kernel::compile_items_impl(self.items.clone(), self.end, "", layouts, backend, modules, None) {
            Ok(kernel) => {
                let kernel = KernelRef::from_arc(kernel);
                COMPILED.lock().unwrap_or_else(|e| e.into_inner()).insert(key, kernel.clone());
                Ok(kernel)
            }
            Err(errors) => Err(errors.into_iter().map(|e| {
                let place = self.locate(e.start);
                (e, place)
            }).collect()),
        }
    }

    /// Where each compiled-in fn is written: file, 1-based first line and
    /// column, last line and column (one past its last token).
    pub(crate) fn fn_places(&self) -> Vec<(String, u32, u32, u32, u32)> {
        self.places.iter().filter_map(|(file, toks)| {
            let (first, last) = (toks.first()?, toks.last()?);
            Some((file.clone(), first.2, first.3, last.2, last.3 + (last.1 - last.0) as u32))
        }).collect()
    }

    /// What a compile depends on, hashed: the syntax tree (with its spans),
    /// the layouts, the backend and the modules.
    fn key(&self, layouts: &[Layout], backend: Backend, modules: &[crate::module::Module]) -> u64 {
        use std::fmt::Write;
        struct Fnv(u64);
        impl Write for Fnv {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                for b in s.bytes() {
                    self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
                }
                Ok(())
            }
        }
        let mut h = Fnv(0xcbf2_9ce4_8422_2325);
        let _ = write!(h, "{:?}\u{0}{}\u{0}{:?}\u{0}{:?}", self.items, self.end, layouts, backend);
        for m in modules {
            let _ = write!(h, "\u{0}{}\u{0}{}", m.path, m.source);
        }
        h.0
    }
}

/// The kernels compiled in this process, by [`KernelProgram::key`].
static COMPILED: std::sync::Mutex<std::collections::BTreeMap<u64, KernelRef>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// A constant a kernel can hold, from a document value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Const {
    Num(f64),
    /// An integer table entry.
    Int(i64),
    Color(u32),
    /// vec2 / vec3 / vec4: the lane count and the lanes.
    Vec(u8, [f32; 4]),
}

/// The constant a value holds: a finite number, a colour or a float vector.
fn constant(vm: &ScriptVm, v: ScriptValue) -> Option<Const> {
    if let Some(c) = v.as_color() {
        return Some(Const::Color(c));
    }
    if let Some(x) = v.as_number() {
        return x.is_finite().then_some(Const::Num(x));
    }
    let pod = v.as_pod()?;
    let (ty, data) = vm.heap().pod_data(pod);
    let n = match ty.ty().as_vec() {
        Some(pod::ScriptPodVec::Vec2f) => 2,
        Some(pod::ScriptPodVec::Vec3f) => 3,
        Some(pod::ScriptPodVec::Vec4f) => 4,
        _ => return None,
    };
    let mut lanes = [0f32; 4];
    for (k, w) in data.iter().take(n).enumerate() {
        lanes[k] = f32::from_bits(*w);
        if !lanes[k].is_finite() {
            return None;
        }
    }
    (data.len() >= n).then_some(Const::Vec(n as u8, lanes))
}

const NOWHERE: Span = Span { start: 0, end: 0 };

/// A document value as the kernel constant it is, written out: a number
/// (an integer when the document wrote one), colour, vector, array or plain
/// object (nested); a part a kernel cannot hold (a string, an object made
/// from a prototype) is unreadable, an error only where a kernel reads it.
fn value_expr(vm: &ScriptVm, v: ScriptValue, depth: usize) -> Option<Expr> {
    if depth > 16 {
        return None;
    }
    if v.is_u40() {
        return Some(num(v.as_number()?, true));
    }
    if let Some(c) = constant(vm, v) {
        return Some(const_expr(c));
    }
    let unreadable = |why: &str| Some(expr(ExprKind::Unreadable(why.to_string())));
    if vm.heap().string_with(v, |_, _| ()).is_some() {
        return unreadable("a string: kernels have no strings");
    }
    if let Some(a) = v.as_array() {
        let n = vm.heap().array_len(a);
        let mut items = Vec::with_capacity(n);
        for i in 0..n {
            items.push(value_expr(vm, vm.heap().array_index(a, i, NoTrap), depth + 1)?);
        }
        return Some(expr(ExprKind::ArrayList(items)));
    }
    let o = v.as_object()?;
    if vm.heap().as_fn(o).is_some() {
        return None;
    }
    if vm.heap().proto(o).as_object().is_some() {
        return unreadable("an object made from a prototype (`Name{..}`): a kernel sees only plain objects (`{a: 1}`), whose fields are all written out");
    }
    let mut fields = Vec::new();
    for e in vm.heap().map_entries_ordered(o) {
        let Some(k) = e.key.as_id() else { continue };
        let value = match value_expr(vm, e.value, depth + 1) {
            Some(x) => x,
            None => expr(ExprKind::Unreadable("a value kernel code cannot hold".into())),
        };
        fields.push((k.to_string(), value));
    }
    Some(expr(ExprKind::StructLit(String::new(), fields)))
}

fn expr(kind: ExprKind) -> Expr {
    Expr { kind, span: NOWHERE }
}

/// A number literal (`-x` is a negation of `x`, as the kernel language reads it).
fn num(x: f64, int: bool) -> Expr {
    if x.is_sign_negative() {
        expr(ExprKind::Neg(Box::new(expr(ExprKind::Num(-x, int)))))
    } else {
        expr(ExprKind::Num(x, int))
    }
}

fn const_expr(c: Const) -> Expr {
    match c {
        Const::Num(x) => num(x, false),
        Const::Int(x) => num(x as f64, true),
        Const::Color(c) => expr(crate::parse::color_value(c, NOWHERE)),
        Const::Vec(n, lanes) => expr(ExprKind::Call(format!("vec{n}"), lanes[..n as usize].iter().map(|x| num(*x as f64, false)).collect())),
    }
}

fn ident(name: &str) -> Expr {
    expr(ExprKind::Ident(name.to_string()))
}

fn call(f: &str, args: Vec<Expr>) -> Expr {
    expr(ExprKind::Call(f.to_string(), args))
}

fn let_item(name: &str, ann: Option<TypeAnn>, value: Expr) -> Item {
    Item::Let { name: name.to_string(), ann, value, span: NOWHERE }
}

/// The declaration as the kernel language item it is
/// (`let name = input(ty, stride, offset, buffer)`, …).
fn decl_item(d: &Decl) -> Result<Item, ShaderError> {
    let whole = |x: u32| num(x as f64, true);
    Ok(match d {
        Decl::Input { name, ty, stride, offset, buffer } | Decl::Output { name, ty, stride, offset, buffer } => {
            let f = if matches!(d, Decl::Input { .. }) { "input" } else { "output" };
            let mut args = vec![ident(ty)];
            if stride.is_some() || offset.is_some() || buffer.is_some() {
                args.push(whole(stride.unwrap_or(0)));
            }
            if offset.is_some() || buffer.is_some() {
                args.push(whole(offset.unwrap_or(0)));
            }
            if let Some(b) = buffer {
                args.push(ident(b));
            }
            let_item(name, None, call(f, args))
        }
        Decl::EmitBuffer { name, record, capacity, buffer } => {
            let mut args = vec![
                match record {
                    Record::Type(t) => ident(t),
                    Record::Words(w) => whole(*w),
                },
                whole(*capacity),
            ];
            if let Some(b) = buffer {
                args.push(ident(b));
            }
            let_item(name, None, call("emit_buffer", args))
        }
        Decl::Param { name, default, range } => {
            let mut args = vec![num(*default as f64, false)];
            if let Some((lo, hi)) = range {
                args.push(num(*lo as f64, false));
                args.push(num(*hi as f64, false));
            }
            let_item(name, None, call("param", args))
        }
        Decl::Struct { name, fields } => Item::Struct { name: name.clone(), fields: fields.iter().map(|(k, c)| (k.clone(), const_expr(*c))).collect(), span: NOWHERE },
        Decl::Table { name, ty, values } => {
            let ann = crate::parse::type_ann_named(ty)
                .ok_or_else(|| ShaderError::new(0, 1, format!("unknown type `{ty}` (f32, f64, i32, bool, vec2, vec3, vec4, mat4)")))?;
            let_item(name, Some(ann), expr(ExprKind::ArrayList(values.iter().map(|c| const_expr(*c)).collect())))
        }
    })
}

/// A script fn's tokens as the kernel language's: one per VM token (an
/// operator the VM reads as one, such as `=-`, is the kernel language's
/// `=` `-`; `@name` is one name), spans from `base` on, ended by an end
/// token; and each token's document place.
fn kernel_tokens(vm: &ScriptVm, f: &ScriptFnTokens, base: usize) -> Result<(Vec<Token>, Vec<(usize, usize, u32, u32)>), ShaderError> {
    use makepad_script::tokenizer::ScriptToken as St;
    let origin = f.tokens.first().map_or(0, |t| t.start);
    let mut out: Vec<Token> = Vec::with_capacity(f.tokens.len() + 1);
    let mut places = Vec::with_capacity(f.tokens.len());
    let mut glue_name = false;
    for t in &f.tokens {
        let (start, end) = (base + (t.start - origin), base + (t.end.max(t.start + 1) - origin));
        places.push((start, end, t.line, t.col));
        let at = |message: String| ShaderError::new(start, end, message);
        match t.token {
            St::Identifier(id) => {
                let name = id.to_string();
                if glue_name {
                    // `@name`: the `@` before it, written against it.
                    let prev = out.pop().expect("the `@` token");
                    push(&mut out, Tk::Ident(format!("@{name}")), prev.start, end, prev.nl);
                } else {
                    push(&mut out, Tk::Ident(name), start, end, t.newline);
                }
            }
            St::Operator(id) | St::Separator(id) => {
                let text = id.to_string();
                let mut rest = text.as_str();
                let mut s = start;
                let mut nl = t.newline;
                while !rest.is_empty() {
                    if rest == "@" {
                        push(&mut out, Tk::Punct("@"), s, s + 1, nl);
                        break;
                    }
                    let Some(p) = crate::parse::PUNCTS.iter().find(|p| rest.starts_with(**p)) else {
                        return Err(ShaderError::new(s, s + 1, format!("unexpected character `{}`", rest.chars().next().unwrap_or(' '))));
                    };
                    push(&mut out, Tk::Punct(p), s, s + p.len(), nl);
                    s += p.len();
                    nl = false;
                    rest = &rest[p.len()..];
                }
            }
            St::OpenCurly => push(&mut out, Tk::Punct("{"), start, end, t.newline),
            St::CloseCurly => push(&mut out, Tk::Punct("}"), start, end, t.newline),
            St::OpenRound => push(&mut out, Tk::Punct("("), start, end, t.newline),
            St::CloseRound => push(&mut out, Tk::Punct(")"), start, end, t.newline),
            St::OpenSquare => push(&mut out, Tk::Punct("["), start, end, t.newline),
            St::CloseSquare => push(&mut out, Tk::Punct("]"), start, end, t.newline),
            St::U40(v) => push(&mut out, Tk::Num(v as f64, true), start, end, t.newline),
            St::F64(v) => push(&mut out, Tk::Num(v, false), start, end, t.newline),
            St::Color(c) => push(&mut out, Tk::Color(c), start, end, t.newline),
            St::String(v) => {
                let s = vm.heap().string_with(v, |_, s| s.to_string()).unwrap_or_default();
                push(&mut out, Tk::Str(s), start, end, t.newline)
            }
            St::F32(_) | St::U32(_) | St::I32(_) | St::F16(_) => return Err(at("a typed number literal: kernels read plain numbers".into())),
            St::RustValue(_) => return Err(at("a value injected from Rust: kernels read document values".into())),
            St::StringUnfinished | St::End | St::StreamEnd => return Err(at("unclosed or overlong string".into())),
        }
        glue_name = matches!(out.last(), Some(Token { tk: Tk::Punct("@"), end: e, .. }) if *e == end) && matches!(t.token, St::Operator(_));
    }
    let end = out.last().map_or(base, |t| t.end);
    push(&mut out, Tk::Eof, end, end, true);
    Ok((out, places))
}

fn push(out: &mut Vec<Token>, tk: Tk, start: usize, end: usize, nl: bool) {
    out.push(Token { tk, start, end, nl });
}

/// The names a fn uses that it does not bind itself (a parameter, a `let`
/// or `var` before the use, a loop variable, a local fn), in first-use
/// order, each with the place of its first use and whether that use calls
/// it (`f(..)`: a fn's name, never a constant). Scopes are the language's:
/// `let x = x + 1` reads the `x` outside.
fn free_names(f: &FnDecl) -> Vec<(String, Span, bool)> {
    struct Walk<'a> {
        scopes: Vec<Vec<&'a str>>,
        out: Vec<(String, Span, bool)>,
    }
    impl<'a> Walk<'a> {
        fn bound(&self, name: &str) -> bool {
            self.scopes.iter().any(|s| s.contains(&name))
        }
        fn use_name(&mut self, name: &'a str, span: Span, called: bool) {
            // `physics.nearest(..)`: a module's fn; the name is its root.
            let root = name.split('.').next().unwrap_or(name);
            if !self.bound(root) && !self.out.iter().any(|(n, _, _)| n == root) {
                self.out.push((root.to_string(), span, called));
            }
        }
        fn bind(&mut self, name: &'a str) {
            if let Some(s) = self.scopes.last_mut() {
                s.push(name);
            }
        }
        fn fn_decl(&mut self, f: &'a FnDecl) {
            self.scopes.push(f.params.iter().map(|(p, _)| p.as_str()).collect());
            self.stmts(&f.body);
            self.scopes.pop();
        }
        fn block(&mut self, body: &'a [Stmt]) {
            self.scopes.push(Vec::new());
            self.stmts(body);
            self.scopes.pop();
        }
        fn stmts(&mut self, body: &'a [Stmt]) {
            for st in body {
                match st {
                    Stmt::Let { name, value, .. } => {
                        // A local fn sees itself; any other value the name outside.
                        if let ExprKind::Lambda(_) = value.kind {
                            self.bind(name);
                            self.expr(value);
                        } else {
                            self.expr(value);
                            self.bind(name);
                        }
                    }
                    Stmt::Assign { target, value, .. } => {
                        self.expr(target);
                        self.expr(value);
                    }
                    Stmt::Expr(e) => self.expr(e),
                    Stmt::For { var, from, to, body, .. } => {
                        self.expr(from);
                        self.expr(to);
                        self.scopes.push(vec![var.as_str()]);
                        self.stmts(body);
                        self.scopes.pop();
                    }
                    Stmt::While { cond, body, .. } => {
                        self.expr(cond);
                        self.block(body);
                    }
                    Stmt::Loop { body, .. } => self.block(body),
                    Stmt::Return(Some(e), _) => self.expr(e),
                    Stmt::Return(None, _) | Stmt::Break(_) | Stmt::Continue(_) => {}
                }
            }
        }
        fn expr(&mut self, e: &'a Expr) {
            match &e.kind {
                ExprKind::Ident(name) => self.use_name(name, e.span, false),
                ExprKind::Call(name, args) => {
                    self.use_name(name, e.span, true);
                    for a in args {
                        self.expr(a);
                    }
                }
                ExprKind::Field(x, _) | ExprKind::Neg(x) | ExprKind::Not(x) => self.expr(x),
                ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => {
                    self.expr(a);
                    self.expr(b);
                }
                ExprKind::If(arms, otherwise) => {
                    for (cond, body) in arms {
                        self.expr(cond);
                        self.block(body);
                    }
                    if let Some(body) = otherwise {
                        self.block(body);
                    }
                }
                ExprKind::Match(x, arms) => {
                    self.expr(x);
                    for (patterns, body) in arms {
                        for p in patterns.iter().flatten() {
                            self.expr(p);
                        }
                        self.block(body);
                    }
                }
                ExprKind::Block(body) => self.block(body),
                ExprKind::ArrayList(items) => {
                    for x in items {
                        self.expr(x);
                    }
                }
                // `Segment{a: ..}`: the name is a record type's.
                ExprKind::StructLit(_, fields) => {
                    for (_, x) in fields {
                        self.expr(x);
                    }
                }
                ExprKind::Lambda(f) => self.fn_decl(f),
                ExprKind::Num(..) | ExprKind::Bool(_) | ExprKind::Unreadable(_) => {}
            }
        }
    }
    let mut w = Walk { scopes: Vec::new(), out: Vec::new() };
    w.fn_decl(f);
    w.out
}

/// Gives a constant's every part the place where a kernel reads it: an
/// error in it (a string read) points there.
fn respan(e: &mut Expr, span: Span) {
    e.span = span;
    match &mut e.kind {
        ExprKind::Neg(x) => respan(x, span),
        ExprKind::Call(_, items) | ExprKind::ArrayList(items) => items.iter_mut().for_each(|x| respan(x, span)),
        ExprKind::StructLit(_, fields) => fields.iter_mut().for_each(|(_, x)| respan(x, span)),
        _ => {}
    }
}

/// The kernel program for `k`: its declarations, its entry and every
/// document fn and constant the entry reaches, as the compiler's syntax
/// tree.
/// `whole_values`: the document's values as it wrote them (a document
/// kernel host's choice): whole numbers stay integers, and arrays and plain
/// objects are constants too (else numbers are floats and only numbers,
/// colours and vectors are).
pub(crate) fn kernel_program(vm: &ScriptVm, k: &VmKernel, whole_values: bool) -> Result<KernelProgram, ShaderError> {
    let fn_tokens = |f: ScriptObject| vm.heap().fn_script_ip(f).and_then(|ip| vm.fn_tokens_placed(ip));
    let mut items = Vec::new();
    if k.math == MathMode::Portable {
        items.push(let_item("math", None, ident("portable")));
    }
    for m in &k.uses {
        items.push(Item::Use { path: m.clone(), glob: true, alias: None, span: NOWHERE });
    }
    let mut known: HashSet<String> = k.decls.iter().map(|d| d.name().to_string()).collect();
    known.insert(k.entry.name().to_string());
    // Every kernel's own inputs, and the compiler's constants (PI, TAU, E).
    for name in ["time", "seed", "count"].iter().chain(crate::lower::BUILTIN_NAMES) {
        known.insert(name.to_string());
    }
    known.extend(k.bind.iter().map(|(name, _)| name.clone()));
    for d in &k.decls {
        items.push(decl_item(d)?);
    }
    // The entry, then what it reaches (depth first from the last found;
    // each name once).
    let Some(tokens) = fn_tokens(k.entry_fn) else {
        return Err(ShaderError::new(0, 1, format!("the {} entry is not a script fn", k.entry.name())));
    };
    let mut todo = vec![(k.entry_fn, k.entry.name().to_string(), tokens)];
    for (name, f) in k.bind.iter().rev() {
        let Some(tokens) = fn_tokens(*f) else {
            return Err(ShaderError::new(0, 1, format!("`{name}` is not a script fn")));
        };
        todo.push((*f, name.clone(), tokens));
    }
    let mut emitted: HashSet<String> = HashSet::new();
    let mut consts = Vec::new();
    let mut fns = Vec::new();
    let mut places = Vec::new();
    let mut error = None;
    let mut free: Vec<String> = Vec::new();
    let mut constants = Vec::new();
    // Span 0 is the declarations' and constants' (no document place).
    let mut base = 1;
    while let Some((f, name, tokens)) = todo.pop() {
        emitted.insert(name.clone());
        let (toks, place) = kernel_tokens(vm, &tokens, base).map_err(|e| ShaderError::new(e.start, e.end, format!("{name}: {}", e.message)))?;
        base = toks.last().map_or(base, |t| t.end) + 1;
        let parsed = if tokens.closure { Parser::new(&toks).closure_value(name.clone()) } else { Parser::new(&toks).fn_value(name.clone()) };
        let names = match parsed {
            Ok(decl) => {
                let names = free_names(&decl);
                fns.push(Item::Fn(decl));
                names
            }
            Err(e) => {
                error.get_or_insert((e.start, e.end, e.message));
                Vec::new()
            }
        };
        places.push((tokens.file.clone(), place));
        for (id, span, called) in names {
            if !free.contains(&id) {
                free.push(id.clone());
            }
            if known.contains(&id) || emitted.contains(&id) {
                continue;
            }
            let value = vm.heap().scope_value(f, LiveId::from_str(&id), NoTrap);
            if value.is_nil() || value.is_err() {
                continue;
            }
            if let Some(obj) = value.as_object().filter(|o| vm.heap().as_fn(*o).is_some()) {
                if let Some(tokens) = fn_tokens(obj) {
                    known.insert(id.clone());
                    todo.push((obj, id, tokens));
                }
                continue;
            }
            // A called name is a fn's (the compiler's, a module's): never a constant.
            if called {
                continue;
            }
            let value = if whole_values { value_expr(vm, value, 0) } else { constant(vm, value).map(const_expr) };
            if let Some(mut e) = value {
                respan(&mut e, span);
                known.insert(id.clone());
                constants.push(id.clone());
                consts.push(let_item(&id, None, e));
            }
        }
    }
    // Constants go first (a kernel's lets are in order); the fns last.
    consts.extend(items);
    consts.extend(fns);
    // A free name that is a compiled-in fn is not free.
    free.retain(|n| !emitted.contains(n));
    Ok(KernelProgram { items: consts, end: base, places, error, free, constants })
}

/// Compiles a kernel made from Splash values (see the module docs). The
/// errors are the compiler's, their messages naming the document place.
/// The host's declarations and layouts cross as borrowed views; the
/// program is built and compiled inside the compiler.
#[inline(always)]
pub fn compile(vm: &ScriptVm, k: &VmKernel, layouts: &[crate::boundary::LayoutView<'_>], backend: Backend, modules: &[crate::module::Module]) -> Result<KernelRef, Vec<ShaderError>> {
    use crate::boundary::{DeclKind, DeclView, VmKernelView};
    let mut decls = Vec::with_capacity(k.decls.len());
    for d in &k.decls {
        let none = DeclView { kind: DeclKind::Param, name: d.name(), ty: "", stride: u32::MAX, offset: u32::MAX, buffer: "", words: 0, capacity: 0, default: 0.0, min: 0.0, max: 0.0, ranged: false, record: &[] };
        decls.push(match d {
            Decl::Input { ty, stride, offset, buffer, .. } | Decl::Output { ty, stride, offset, buffer, .. } => DeclView {
                kind: if matches!(d, Decl::Input { .. }) { DeclKind::Input } else { DeclKind::Output },
                ty,
                stride: stride.unwrap_or(u32::MAX),
                offset: offset.unwrap_or(u32::MAX),
                buffer: buffer.as_deref().unwrap_or(""),
                ..none
            },
            Decl::EmitBuffer { record, capacity, buffer, .. } => DeclView {
                kind: DeclKind::EmitBuffer,
                ty: match record { Record::Type(t) => t, Record::Words(_) => "" },
                words: match record { Record::Type(_) => 0, Record::Words(w) => *w },
                capacity: *capacity,
                buffer: buffer.as_deref().unwrap_or(""),
                ..none
            },
            Decl::Param { default, range, .. } => DeclView {
                default: *default,
                min: range.map_or(0.0, |r| r.0),
                max: range.map_or(0.0, |r| r.1),
                ranged: range.is_some(),
                ..none
            },
            // A host's own kernel declares buffers and params; records and
            // tables are a document's (a Kernel object, read compiler-side).
            Decl::Struct { .. } | Decl::Table { .. } => {
                return Err(vec![ShaderError::new(0, 1, format!("`{}`: record and table declarations come from a Kernel object", d.name()))]);
            }
        });
    }
    let uses: Vec<&str> = k.uses.iter().map(|u| u.as_str()).collect();
    let bind_names: Vec<&str> = k.bind.iter().map(|(n, _)| n.as_str()).collect();
    let bind_fns: Vec<ScriptObject> = k.bind.iter().map(|(_, f)| *f).collect();
    let view = VmKernelView { decls: &decls, entry: k.entry, entry_fn: k.entry_fn, math: k.math, uses: &uses, bind_names: &bind_names, bind_fns: &bind_fns, whole_values: false };
    let mut compiled = crate::boundary::compile_vm_kernel(vm, &view, layouts, backend, modules);
    if let Some(kernel) = compiled.take_kernel() {
        return Ok(kernel);
    }
    Err((0..compiled.error_count()).map(|index| {
        let error = compiled.error(index);
        ShaderError::new(error.start, error.end, error.message.to_string())
    }).collect())
}

// ---------------------------------------------------------------------------
// Kernels as Splash objects
// ---------------------------------------------------------------------------

/// `mod.kernel`: what a document writes a kernel with, as values the VM
/// evaluates (no host reads the text):
///
/// ```splash
/// use mod.kernel.*
/// let sparks = {
///     out: output(Sprite)            // input(ty [, stride, offset, "buffer"])
///     path: emit_buffer(vec3, 420)   // emit_buffer(ty or words, slots [, "buffer"])
///     amp: param(1.0, 0.0, 10.0)     // param(default [, min, max])
///     element: fn(i) { out[i].pos = vec3(float(i) * amp, 0.0, 0.0) }
/// }
/// ```
///
/// Element types are the pod types (`f32`, `vec3`, …) or a host layout,
/// which this registers by name (`Sprite` above). [`from_object`] reads such
/// an object back.
#[inline(always)]
pub fn script_mod(vm: &mut ScriptVm, layouts: &[crate::boundary::LayoutView<'_>]) {
    let names: Vec<&str> = layouts.iter().map(|layout| layout.name).collect();
    script_mod_names(vm, &names);
}

/// Registers host layout names without borrowing the host's String/Vec layouts.
pub fn script_mod_names(vm: &mut ScriptVm, names: &[&str]) {
    let m = match vm.value(vm.modules(), id!(kernel)).as_object() {
        Some(m) => m,
        None => vm.new_module(id!(kernel)),
    };
    for name in names {
        let o = vm.new_object();
        vm.set_value_def(o, id!(kernel_layout), LiveId::from_str(name));
        vm.set_value_def(m, LiveId::from_str(name), o);
    }
    fn decl(vm: &mut ScriptVm, kind: LiveId, args: ScriptObject, keys: &[LiveId]) -> ScriptValue {
        let o = vm.new_object();
        vm.set_value_def(o, id!(kernel_decl), kind);
        for k in keys {
            let v = vm.value(args, *k);
            vm.set_value_def(o, *k, v);
        }
        o.into()
    }
    vm.add_method(m, id_lut!(input), script_args!(ty = NIL, stride = NIL, offset = NIL, buffer = NIL), |vm, args| {
        decl(vm, id!(input), args, &[id!(ty), id!(stride), id!(offset), id!(buffer)])
    });
    vm.add_method(m, id_lut!(output), script_args!(ty = NIL, stride = NIL, offset = NIL, buffer = NIL), |vm, args| {
        decl(vm, id!(output), args, &[id!(ty), id!(stride), id!(offset), id!(buffer)])
    });
    vm.add_method(m, id_lut!(emit_buffer), script_args!(ty = NIL, slots = NIL, buffer = NIL), |vm, args| {
        decl(vm, id!(emit_buffer), args, &[id!(ty), id!(slots), id!(buffer)])
    });
    vm.add_method(m, id_lut!(param), script_args!(default = NIL, min = NIL, max = NIL), |vm, args| {
        decl(vm, id!(param), args, &[id!(default), id!(min), id!(max)])
    });
    // `record({a: 0.0 b: vec3(0.0)})`: a record type, its fields' zero
    // values in order.
    vm.add_method(m, id_lut!(record), script_args!(fields = NIL), |vm, args| decl(vm, id!(record), args, &[id!(fields)]));
    // `table(i32, [0, 1, 3])`: a constant table.
    vm.add_method(m, id_lut!(table), script_args!(ty = NIL, values = NIL), |vm, args| decl(vm, id!(table), args, &[id!(ty), id!(values)]));
}

/// An element type value's name: a pod type, a registered layout, or a
/// record type the kernel declares, by name (`@Seg`).
fn type_name(vm: &ScriptVm, v: ScriptValue) -> Option<String> {
    if let Some(id) = v.as_id() {
        return Some(id.to_string());
    }
    if let Some(o) = v.as_object() {
        if let Some(id) = vm.value(o, id!(kernel_layout)).as_id() {
            return Some(id.to_string());
        }
    }
    let ty = vm.heap().pod_type(v)?;
    use makepad_script::pod::{ScriptPodTyTag as P, ScriptPodVec as V, ScriptPodMat};
    let ty = vm.heap().pod_type_ref(ty).ty();
    Some(match ty.kind() {
        P::F32 => "f32",
        P::I32 => "i32",
        P::U32 => "u32",
        P::Vec => match ty.as_vec()? {
            V::Vec2f => "vec2", V::Vec3f => "vec3", V::Vec4f => "vec4", _ => return None,
        },
        P::Mat if ty.as_mat() == Some(ScriptPodMat::Mat4x4f) => "mat4",
        _ => return None,
    }.to_string())
}

/// The declaration a value of `mod.kernel` makes under `name` (`input(..)`,
/// `output(..)`, `emit_buffer(..)`, `param(..)`, `record(..)`, `table(..)`);
/// None for any other value.
pub fn read_decl(vm: &ScriptVm, name: &str, v: ScriptValue) -> Result<Option<Decl>, String> {
    let Some(o) = v.as_object() else { return Ok(None) };
    let Some(kind) = vm.value(o, id!(kernel_decl)).as_id() else { return Ok(None) };
    let name = name.to_string();
    let get = |k: LiveId| vm.value(o, k);
    let count = |k: LiveId| -> Result<Option<u32>, String> {
        let v = get(k);
        if v.is_nil() {
            return Ok(None);
        }
        match v.as_number() {
            Some(x) if x >= 0.0 && x.fract() == 0.0 && x < 16_777_216.0 => Ok(Some(x as u32)),
            _ => Err(format!("{name}: {k} is a whole number")),
        }
    };
    let buffer = || vm.heap().string_with(get(id!(buffer)), |_, s| s.to_string());
    let number = |k: LiveId| get(k).as_number().map(|x| x as f32);
    Ok(Some(if kind == id!(input) || kind == id!(output) {
        let ty = type_name(vm, get(id!(ty))).ok_or_else(|| format!("{name}: the element type (f32, i32, u32, vec2, vec3, vec4, mat4, a layout or a record @Name)"))?;
        let (stride, offset, buffer) = (count(id!(stride))?, count(id!(offset))?, buffer());
        if kind == id!(input) {
            Decl::Input { name, ty, stride, offset, buffer }
        } else {
            Decl::Output { name, ty, stride, offset, buffer }
        }
    } else if kind == id!(emit_buffer) {
        let record = match type_name(vm, get(id!(ty))) {
            Some(t) => Record::Type(t),
            None => Record::Words(count(id!(ty))?.ok_or_else(|| format!("{name}: the record type or width"))?),
        };
        let capacity = count(id!(slots))?.ok_or_else(|| format!("{name}: the slots per element"))?;
        Decl::EmitBuffer { name, record, capacity, buffer: buffer() }
    } else if kind == id!(record) {
        let f = get(id!(fields)).as_object().ok_or_else(|| format!("{name}: record({{field: zero value ..}})"))?;
        let mut fields = Vec::new();
        let mut err = None;
        for e in vm.heap().map_entries_ordered(f) {
            let (k, v) = (e.key, e.value);
            let Some(k) = k.as_id() else { continue };
            match constant(vm, v) {
                Some(c) => fields.push((k.to_string(), c)),
                None => err = Some(format!("{name}.{k}: a record field's zero value is a number or a vec2/3/4")),
            }
        }
        if let Some(e) = err {
            return Err(e);
        }
        Decl::Struct { name, fields }
    } else if kind == id!(table) {
        let ty = type_name(vm, get(id!(ty))).ok_or_else(|| format!("{name}: table(type, [values])"))?;
        let a = get(id!(values)).as_array().ok_or_else(|| format!("{name}: table(type, [values])"))?;
        let int = ty == "i32" || ty == "u32";
        let mut values = Vec::new();
        for i in 0..vm.heap().array_len(a) {
            let x = vm.heap().array_index(a, i, NoTrap).as_number().ok_or_else(|| format!("{name}[{i}]: a number"))?;
            values.push(if int { Const::Int(x as i64) } else { Const::Num(x) });
        }
        Decl::Table { name, ty, values }
    } else {
        let default = number(id!(default)).ok_or_else(|| format!("{name}: param(default [, min, max]) takes numbers"))?;
        let range = match (number(id!(min)), number(id!(max))) {
            (Some(lo), Some(hi)) => Some((lo, hi)),
            _ => None,
        };
        Decl::Param { name, default, range }
    }))
}

/// The kernel a Splash object declares (see [`script_mod`]): its
/// declaration fields, its one entry fn and an optional `math: "portable"`.
/// Other fields are the host's (a count, inputs it wires itself).
pub fn from_object(vm: &ScriptVm, obj: ScriptObject) -> Result<VmKernel, String> {
    let mut fields = Vec::new();
    for e in vm.heap().map_entries_ordered(obj) {
        let (k, v) = (e.key, e.value);
        if let Some(k) = k.as_id() {
            fields.push((k, v));
        }
    }
    let mut decls = Vec::new();
    let mut entry = None;
    let mut math = MathMode::Fast;
    for (key, v) in fields {
        let name = key.to_string();
        if let Some(e) = Entry::from_name(&name) {
            let Some(f) = v.as_object().filter(|f| vm.heap().as_fn(*f).is_some()) else {
                return Err(format!("`{name}` is the kernel's entry: a fn(i)"));
            };
            if let Some((other, _)) = entry {
                return Err(format!("a kernel has one entry; found {} and {name}", Entry::name(other)));
            }
            entry = Some((e, f));
            continue;
        }
        if name == "math" {
            math = match vm.heap().string_with(v, |_, s| s.to_string()).as_deref() {
                Some("portable") => MathMode::Portable,
                Some("fast") => MathMode::Fast,
                _ => return Err("math: \"portable\" or \"fast\"".into()),
            };
            continue;
        }
        if let Some(d) = read_decl(vm, &name, v)? {
            decls.push(d);
        }
    }
    let (entry, entry_fn) = entry.ok_or("missing kernel entry: element, vertex, instance, primitive or reduce_sum / reduce_min / reduce_max")?;
    Ok(VmKernel { decls, entry, entry_fn, math, uses: Vec::new(), bind: Vec::new() })
}


// Host-side data: trait operations must use the caller's collection layouts.
impl Clone for Decl {
    #[inline(always)]
    fn clone(&self) -> Self {
        match self {
            Self::Input { name, ty, stride, offset, buffer } => Self::Input { name: name.clone(), ty: ty.clone(), stride: stride.clone(), offset: offset.clone(), buffer: buffer.clone() },
            Self::Output { name, ty, stride, offset, buffer } => Self::Output { name: name.clone(), ty: ty.clone(), stride: stride.clone(), offset: offset.clone(), buffer: buffer.clone() },
            Self::EmitBuffer { name, record, capacity, buffer } => Self::EmitBuffer { name: name.clone(), record: record.clone(), capacity: capacity.clone(), buffer: buffer.clone() },
            Self::Param { name, default, range } => Self::Param { name: name.clone(), default: default.clone(), range: range.clone() },
            Self::Struct { name, fields } => Self::Struct { name: name.clone(), fields: fields.clone() },
            Self::Table { name, ty, values } => Self::Table { name: name.clone(), ty: ty.clone(), values: values.clone() },
        }
    }
}

impl PartialEq for Decl {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Input { name: a_name, ty: a_ty, stride: a_stride, offset: a_offset, buffer: a_buffer }, Self::Input { name: b_name, ty: b_ty, stride: b_stride, offset: b_offset, buffer: b_buffer }) => a_name == b_name && a_ty == b_ty && a_stride == b_stride && a_offset == b_offset && a_buffer == b_buffer,
            (Self::Output { name: a_name, ty: a_ty, stride: a_stride, offset: a_offset, buffer: a_buffer }, Self::Output { name: b_name, ty: b_ty, stride: b_stride, offset: b_offset, buffer: b_buffer }) => a_name == b_name && a_ty == b_ty && a_stride == b_stride && a_offset == b_offset && a_buffer == b_buffer,
            (Self::EmitBuffer { name: a_name, record: a_record, capacity: a_capacity, buffer: a_buffer }, Self::EmitBuffer { name: b_name, record: b_record, capacity: b_capacity, buffer: b_buffer }) => a_name == b_name && a_record == b_record && a_capacity == b_capacity && a_buffer == b_buffer,
            (Self::Param { name: a_name, default: a_default, range: a_range }, Self::Param { name: b_name, default: b_default, range: b_range }) => a_name == b_name && a_default == b_default && a_range == b_range,
            (Self::Struct { name: a_name, fields: a_fields }, Self::Struct { name: b_name, fields: b_fields }) => a_name == b_name && a_fields == b_fields,
            (Self::Table { name: a_name, ty: a_ty, values: a_values }, Self::Table { name: b_name, ty: b_ty, values: b_values }) => a_name == b_name && a_ty == b_ty && a_values == b_values,
            _ => false,
        }
    }
}

impl Clone for Record {
    #[inline(always)]
    fn clone(&self) -> Self {
        match self {
            Self::Type (value) => Self::Type (value.clone()),
            Self::Words (value) => Self::Words (value.clone()),
        }
    }
}

impl PartialEq for Record {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Type (a), Self::Type (b)) => a == b,
            (Self::Words (a), Self::Words (b)) => a == b,
            _ => false,
        }
    }
}
