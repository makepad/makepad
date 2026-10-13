//! Built-in formatting macros on the std S2 ABI (coord.md S2): `format_args!`, `format!`,
//! `print!`/`println!`/`eprint!`/`eprintln!`, `write!`/`writeln!`, `panic!`/`unreachable!`/
//! `todo!`/`unimplemented!`, `assert!`/`assert_eq!`/`assert_ne!`, `vec!` and `dbg!`.
//!
//! The parser expands each call into ordinary Rust, the way rustc's own expansion does:
//! `Arguments::new_v1(&[pieces], &[rt::Argument::new_display(&x), ..])`, then
//! `fmt::format(..)`, `io::_print(..)`, `w.write_fmt(..)` or `panicking::panic_at(.., &Location)`.
//! Typeck and lowering see nothing special. The macro's own argument expressions are parsed once
//! with their real spans and spliced into the generated text as `T::Interp` tokens (generated
//! idents `__relax_splice_N`), so their diagnostics point at the user's code.
//!
//! Child module of `parser` (uses its private helpers).

use super::{PErr, PResult, Parser};
use crate::ast::*;
use crate::lexer::{Tok, T};

/// Paths in generated code: crates by identity (resolve from any crate, including core itself).
const CORE: &str = "::__relax_crate_core";
const STD: &str = "::__relax_crate_std";
const SPLICE: &str = "__relax_splice_";

pub(super) fn is_builtin(name: &[u8]) -> bool {
    matches!(
        name,
        b"format_args"
            | b"format"
            | b"print"
            | b"println"
            | b"eprint"
            | b"eprintln"
            | b"write"
            | b"writeln"
            | b"panic"
            | b"unreachable"
            | b"todo"
            | b"unimplemented"
            | b"assert"
            | b"assert_eq"
            | b"assert_ne"
            | b"vec"
            | b"dbg"
    )
}

/// A value an argument slot formats: a parsed expression (borrowed with `&`), or generated
/// text that already is a reference.
#[derive(Clone)]
enum Src {
    Expr(ExprId),
    Ref(String),
    /// an implicit capture `{name}`: `&name`, the name with the format string's position
    /// (hygiene: rustc resolves it in the format string's context)
    Capture(String, u32),
    /// generated text of a value (borrowed with `&`), e.g. nested `Arguments`
    Val(String),
}

enum ArgRef {
    Next,
    Index(usize),
    Name(String),
}

enum CountSpec {
    Implied,
    Is(usize),
    Arg(ArgRef),
    /// `.*`: the next positional argument
    Star,
}

struct Spec {
    fill: char,
    align: u8,
    flags: u32,
    width: CountSpec,
    precision: CountSpec,
    /// rt::Argument constructor
    ctor: &'static str,
}

enum Piece {
    Lit(String),
    Ph(ArgRef, Spec),
}

/// Arguments of a format-style macro call after its leading expressions.
struct FmtCall {
    /// the format string (unescaped) and its token position
    fmt: Option<(String, u32)>,
    /// a non-literal first argument (`panic!(x)` 2018 style)
    first: Option<ExprId>,
    pos: Vec<ExprId>,
    named: Vec<(String, ExprId)>,
}

/// Generated Rust text plus the expressions spliced into it.
pub(super) struct Gen {
    s: String,
    splices: Vec<ExprId>,
    /// generated text standing for source tokens: (offset, origin position, length)
    origins: Vec<(u32, u32, u32)>,
}

impl Gen {
    fn new() -> Gen {
        Gen { s: String::new(), splices: Vec::new(), origins: Vec::new() }
    }
    pub(super) fn from_text(s: String) -> Gen {
        Gen { s, splices: Vec::new(), origins: Vec::new() }
    }
    fn p(&mut self, x: &str) {
        self.s.push_str(x);
    }
    fn expr(&mut self, e: ExprId) {
        self.s.push_str(&format!(" {}{} ", SPLICE, self.splices.len()));
        self.splices.push(e);
    }
    /// `&value` / the reference itself
    fn borrow(&mut self, s: &Src) {
        match s {
            Src::Expr(e) => {
                self.p("&");
                self.expr(*e);
            }
            Src::Ref(t) => self.p(t),
            Src::Capture(n, at) => {
                self.p("& ");
                self.origins.push((self.s.len() as u32, *at, n.len() as u32));
                self.p(n);
                self.p(" ");
            }
            Src::Val(t) => {
                self.p("&(");
                self.p(t);
                self.p(")");
            }
        }
    }
}

fn str_lit(s: &str) -> String {
    format!("{:?}", s)
}

fn location() -> String {
    // the caller's location inside a #[track_caller] fn, else this site (rustc panic! uses
    // Location::caller())
    format!("{}::panic::Location::caller()", CORE)
}

impl<'a> Parser<'a> {
    /// Expands a built-in formatting macro called in expression position (S2 std only).
    /// None: not one of ours, or the legacy std (old built-in path).
    pub(super) fn builtin_mac_expr(&mut self, path: &Path, toks: TokRange, lo: u32) -> PResult<Option<ExprId>> {
        let s2 = match &self.macros {
            Some(m) => m.table.s2_fmt,
            None => false,
        };
        if !s2 || path.segs.len() > 3 {
            return Ok(None);
        }
        let (nlo, nhi) = match self.ast.segs(path.segs).last() {
            Some(s) => (s.name.lo, s.name.hi),
            None => return Ok(None),
        };
        let name = self.bytes(nlo, nhi).to_vec();
        if !is_builtin(&name) {
            return Ok(None);
        }
        let name = String::from_utf8_lossy(&name).into_owned();
        let save = self.pos;
        self.pos = toks.lo as usize;
        let r = self.builtin_gen(&name, toks.hi as usize, lo);
        self.pos = save;
        let g = r?;
        let start = self.splice_expansion(&name, &g, lo)?;
        Ok(Some(self.in_expansion(start, |p| p.expr())?))
    }

    fn builtin_gen(&mut self, name: &str, hi: usize, lo: u32) -> PResult<Gen> {
        crate::preload::timed_named(3, name, || self.builtin_gen_inner(name, hi, lo))
    }

    fn builtin_gen_inner(&mut self, name: &str, hi: usize, lo: u32) -> PResult<Gen> {
        let mut g = Gen::new();
        match name {
            "format_args" => {
                let c = self.fmt_call(hi, false)?;
                self.gen_args(&mut g, c, Vec::new(), Vec::new(), "")?;
            }
            "format" => {
                let c = self.fmt_call(hi, false)?;
                g.p(&format!("{}::fmt::format(", CORE));
                self.gen_args(&mut g, c, Vec::new(), Vec::new(), "")?;
                g.p(")");
            }
            "print" | "println" | "eprint" | "eprintln" => {
                let c = self.fmt_call(hi, false)?;
                let f = if name.starts_with('e') { "_eprint" } else { "_print" };
                g.p(&format!("{}::io::{}(", STD, f));
                self.gen_args(&mut g, c, Vec::new(), Vec::new(), if name.ends_with("ln") { "\n" } else { "" })?;
                g.p(")");
            }
            "write" | "writeln" => {
                if self.pos >= hi {
                    return Err(PErr { pos: lo, msg: format!("`{}!` needs a destination -- `{}!(f, \"..\")`", name, name) });
                }
                let dst = self.expr()?;
                if self.pos < hi {
                    self.expect(T::Comma)?;
                }
                let c = self.fmt_call(hi, false)?;
                g.expr(dst);
                g.p(".write_fmt(");
                self.gen_args(&mut g, c, Vec::new(), Vec::new(), if name == "writeln" { "\n" } else { "" })?;
                g.p(")");
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                let c = self.fmt_call(hi, true)?;
                let prefix = match name {
                    "unreachable" => "internal error: entered unreachable code",
                    "todo" => "not yet implemented",
                    "unimplemented" => "not implemented",
                    _ => "",
                };
                g.p(&format!("{}::panicking::panic_at(", CORE));
                let empty = c.fmt.is_none() && c.first.is_none();
                if empty {
                    let msg = if prefix.is_empty() { "explicit panic" } else { prefix };
                    g.p(&format!("{}::fmt::Arguments::new_const(&[{}])", CORE, str_lit(msg)));
                } else if prefix.is_empty() {
                    self.gen_args(&mut g, c, Vec::new(), Vec::new(), "")?;
                } else {
                    // "<prefix>: <message>"
                    let mut inner = Gen::new();
                    std::mem::swap(&mut inner.splices, &mut g.splices);
                    self.gen_args(&mut inner, c, Vec::new(), Vec::new(), "")?;
                    std::mem::swap(&mut inner.splices, &mut g.splices);
                    let pieces = vec![Piece::Lit(format!("{}: ", prefix)), Piece::Ph(ArgRef::Index(0), default_spec("new_display"))];
                    self.gen_pieces(&mut g, pieces, vec![Src::Val(inner.s)], lo)?;
                }
                g.p(", ");
                g.p(&location());
                g.p(")");
            }
            "assert" => {
                let cond_lo = self.pos;
                let cond = self.expr()?;
                let cond_hi = self.pos;
                let text = self.tokens_text(cond_lo, cond_hi);
                if self.pos < hi {
                    self.expect(T::Comma)?;
                }
                let c = self.fmt_call(hi, false)?;
                g.p("if !(");
                g.expr(cond);
                g.p(&format!(") {{ {}::panicking::panic_at(", CORE));
                if c.fmt.is_none() {
                    g.p(&format!("{}::fmt::Arguments::new_const(&[{}])", CORE, str_lit(&format!("assertion failed: {}", text))));
                } else {
                    self.gen_args(&mut g, c, Vec::new(), Vec::new(), "")?;
                }
                g.p(", ");
                g.p(&location());
                g.p(") }");
            }
            "assert_eq" | "assert_ne" => {
                let a = self.expr()?;
                self.expect(T::Comma)?;
                let b = self.expr()?;
                if self.pos < hi {
                    self.expect(T::Comma)?;
                }
                let c = self.fmt_call(hi, false)?;
                let eq = name == "assert_eq";
                g.p("match (&");
                g.expr(a);
                g.p(", &");
                g.expr(b);
                g.p(") { (__relax_l, __relax_r) => { if ");
                g.p(if eq { "!(*__relax_l == *__relax_r)" } else { "*__relax_l == *__relax_r" });
                g.p(&format!(" {{ {}::panicking::panic_at(", CORE));
                let op = if eq { "==" } else { "!=" };
                let l = Src::Ref("__relax_l".to_string());
                let r = Src::Ref("__relax_r".to_string());
                let dbg = || default_spec("new_debug");
                let mut pieces = vec![Piece::Lit(format!("assertion `left {} right` failed", op))];
                let mut srcs = Vec::new();
                if c.fmt.is_some() || c.first.is_some() {
                    let mut inner = Gen::new();
                    std::mem::swap(&mut inner.splices, &mut g.splices);
                    self.gen_args(&mut inner, c, Vec::new(), Vec::new(), "")?;
                    std::mem::swap(&mut inner.splices, &mut g.splices);
                    pieces.push(Piece::Lit(": ".to_string()));
                    pieces.push(Piece::Ph(ArgRef::Index(0), default_spec("new_display")));
                    srcs.push(Src::Val(inner.s));
                }
                let n = srcs.len();
                pieces.push(Piece::Lit("\n  left: ".to_string()));
                pieces.push(Piece::Ph(ArgRef::Index(n), dbg()));
                pieces.push(Piece::Lit("\n right: ".to_string()));
                pieces.push(Piece::Ph(ArgRef::Index(n + 1), dbg()));
                srcs.push(l);
                srcs.push(r);
                self.gen_pieces(&mut g, pieces, srcs, lo)?;
                g.p(", ");
                g.p(&location());
                g.p(") } } }");
            }
            "vec" => {
                if self.pos >= hi {
                    g.p(&format!("{}::vec::Vec::new()", CORE));
                } else {
                    let first = self.expr()?;
                    if self.eat(T::Semi) {
                        let n = self.expr()?;
                        g.p(&format!("{}::vec::from_elem(", CORE));
                        g.expr(first);
                        g.p(", ");
                        g.expr(n);
                        g.p(")");
                    } else {
                        let mut v = vec![first];
                        while self.eat(T::Comma) {
                            if self.pos >= hi {
                                break;
                            }
                            v.push(self.expr()?);
                        }
                        // rustc: `<[_]>::into_vec(box [a, b, ..])` -- the expected `Vec<T>`
                        // reaches the elements (each coerces to T: `vec![Box::new(|x| ..), ..]`
                        // as `Vec<Box<dyn Fn>>`)
                        g.p(&format!("<[_]>::into_vec({}::boxed::Box::new([", CORE));
                        for e in v {
                            g.expr(e);
                            g.p(", ");
                        }
                        g.p("]))");
                    }
                }
            }
            "dbg" => {
                // dbg!(e) -> match e { tmp => { eprint!("[file:line:col] text = {:#?}\n", &tmp); tmp } }
                if self.pos >= hi {
                    g.p(&format!("{}::io::_eprint(", STD));
                    let pieces = vec![
                        Piece::Lit("[".to_string()),
                        Piece::Ph(ArgRef::Index(0), default_spec("new_display")),
                        Piece::Lit(":".to_string()),
                        Piece::Ph(ArgRef::Index(1), default_spec("new_display")),
                        Piece::Lit(":".to_string()),
                        Piece::Ph(ArgRef::Index(2), default_spec("new_display")),
                        Piece::Lit("]\n".to_string()),
                    ];
                    let srcs = vec![Src::Val("file!()".to_string()), Src::Val("line!()".to_string()), Src::Val("column!()".to_string())];
                    self.gen_pieces(&mut g, pieces, srcs, lo)?;
                    g.p(")");
                } else {
                    // dbg!(a, b, ..) -> (dbg!(a), dbg!(b), ..,)
                    let mut first = true;
                    let mut multi = false;
                    while self.pos < hi {
                    let e_lo = self.pos;
                    let e = self.expr()?;
                    let text = self.tokens_text(e_lo, self.pos);
                    self.eat(T::Comma);
                    if first {
                        multi = self.pos < hi;
                        if multi {
                            g.p("(");
                        }
                    } else {
                        g.p(", ");
                    }
                    first = false;
                    g.p("match ");
                    g.expr(e);
                    g.p(&format!(" {{ __relax_tmp => {{ {}::io::_eprint(", STD));
                    let mut alt = default_spec("new_debug");
                    alt.flags = 4;
                    let pieces = vec![
                        Piece::Lit("[".to_string()),
                        Piece::Ph(ArgRef::Index(0), default_spec("new_display")),
                        Piece::Lit(":".to_string()),
                        Piece::Ph(ArgRef::Index(1), default_spec("new_display")),
                        Piece::Lit(":".to_string()),
                        Piece::Ph(ArgRef::Index(2), default_spec("new_display")),
                        Piece::Lit(format!("] {} = ", text)),
                        Piece::Ph(ArgRef::Index(3), alt),
                        Piece::Lit("\n".to_string()),
                    ];
                    let srcs = vec![
                        Src::Val("file!()".to_string()),
                        Src::Val("line!()".to_string()),
                        Src::Val("column!()".to_string()),
                        Src::Ref("&__relax_tmp".to_string()),
                    ];
                    self.gen_pieces(&mut g, pieces, srcs, lo)?;
                    g.p("); __relax_tmp } }");
                    if !multi {
                        break;
                    }
                    }
                    if multi {
                        g.p(",)");
                    }
                }
            }
            _ => return Err(PErr { pos: lo, msg: format!("`{}!` is not a built-in macro", name) }),
        }
        if self.pos != hi {
            return self.err(&format!("unexpected tokens in `{}!` arguments", name));
        }
        Ok(g)
    }

    /// Source text of tokens [lo, hi) (for `assert!` / `dbg!` messages), single-spaced
    /// where the source had whitespace.
    fn tokens_text(&self, lo: usize, hi: usize) -> String {
        let mut s = String::new();
        for i in lo..hi.min(self.toks.len()) {
            let t = self.toks.at(i);
            if i > lo {
                let prev = self.toks.at(i - 1);
                if prev.hi != t.lo {
                    s.push(' ');
                }
            }
            s.push_str(&String::from_utf8_lossy(self.tok_text(t)));
        }
        s
    }

    /// `name!(..)` at the cursor, up to the next top-level comma before `hi`, when it is a
    /// built-in that evaluates to a string literal: (its value, the token after it).
    fn fmt_macro_literal(&self, hi: usize) -> Option<(String, usize)> {
        if !(matches!(self.kind(), T::Ident) && self.nth(1) == T::Not) {
            return None;
        }
        let mut depth = 0i32;
        let mut end = hi;
        for i in self.pos..hi {
            match self.toks.at(i).kind {
                T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                T::Comma if depth == 0 => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        match crate::builtins::eval_lit_range(self, self.pos, end) {
            Ok((text, true)) => Some((text, end)),
            _ => None,
        }
    }

    /// `"fmt", args.., name = arg..` up to token `hi`. `loose`: the first argument may be any
    /// expression (`panic!(x)`, formatted with Display).
    fn fmt_call(&mut self, hi: usize, loose: bool) -> PResult<FmtCall> {
        let mut c = FmtCall { fmt: None, first: None, pos: Vec::new(), named: Vec::new() };
        if self.pos >= hi {
            return Ok(c);
        }
        let t = self.tok();
        let lit_alone = matches!(t.kind, T::Str | T::RawStr) && (self.pos + 1 == hi || self.nth(1) == T::Comma);
        if lit_alone {
            self.pos += 1;
            let text = String::from_utf8_lossy(self.tok_text(t)).into_owned();
            c.fmt = Some((crate::lit::unescape_str(&text), t.lo));
        } else if let Some((text, end)) = self.fmt_macro_literal(hi) {
            // a macro that expands to a string literal (`format!(concat!("a", "{}"), x)`):
            // rustc expands it eagerly as the format string (format_args's expr_to_spanned_string)
            c.fmt = Some((text, t.lo));
            self.pos = end;
        } else if loose {
            c.first = Some(self.expr()?);
        } else {
            return Err(PErr { pos: t.lo, msg: "format argument must be a string literal -- write `\"{}\", value`".to_string() });
        }
        while self.pos < hi {
            self.expect(T::Comma)?;
            if self.pos >= hi {
                break;
            }
            if matches!(self.kind(), T::Ident | T::RawIdent) && self.nth(1) == T::Eq {
                let n = self.tok();
                self.pos += 2;
                let mut name = String::from_utf8_lossy(self.tok_text(n)).into_owned();
                if let Some(x) = name.strip_prefix("r#") {
                    name = x.to_string();
                }
                let e = self.expr()?;
                c.named.push((name, e));
            } else {
                if !c.named.is_empty() {
                    return self.err("positional format argument after a named one -- move it before the named ones");
                }
                c.pos.push(self.expr()?);
            }
        }
        Ok(c)
    }

    /// `Arguments` for a parsed call. `suffix` is appended to the format string (`println!`).
    /// `pre_pieces`/`pre_srcs` are unused hooks for generated prefixes (empty here).
    fn gen_args(&mut self, g: &mut Gen, c: FmtCall, mut pieces: Vec<Piece>, mut srcs: Vec<Src>, suffix: &str) -> PResult<()> {
        let base = srcs.len();
        let (fmt, flo) = match (c.fmt, c.first) {
            (Some(f), _) => f,
            (None, Some(e)) => {
                // panic!(value): Display
                pieces.push(Piece::Ph(ArgRef::Index(base), default_spec("new_display")));
                srcs.push(Src::Expr(e));
                if !suffix.is_empty() {
                    pieces.push(Piece::Lit(suffix.to_string()));
                }
                let lo = self.ast.expr(e).lo;
                return self.gen_pieces(g, pieces, srcs, lo);
            }
            (None, None) => (String::new(), 0),
        };
        let mut parsed = parse_format(&fmt).map_err(|m| PErr { pos: flo, msg: m })?;
        if !suffix.is_empty() {
            parsed.push(Piece::Lit(suffix.to_string()));
        }
        // argument slots: positional, then named, then implicit captures
        let npos = c.pos.len();
        for e in &c.pos {
            srcs.push(Src::Expr(*e));
        }
        let mut names: Vec<String> = Vec::new();
        for (n, e) in &c.named {
            names.push(n.clone());
            srcs.push(Src::Expr(*e));
        }
        // rebase indices and names onto slots; captures become new slots
        let mut next = 0usize;
        let mut captures: Vec<String> = Vec::new();
        let mut resolve = |r: &ArgRef, next: &mut usize, srcs: &mut Vec<Src>| -> Result<usize, String> {
            Ok(match r {
                ArgRef::Next => {
                    let i = *next;
                    *next += 1;
                    if i >= npos + names.len() {
                        return Err(format!("format string needs argument {} but there are {} -- add the argument", i + 1, npos + names.len()));
                    }
                    base + i
                }
                ArgRef::Index(i) => {
                    if *i >= npos + names.len() {
                        return Err(format!("format string refers to argument {} but there are {} -- add the argument", i, npos + names.len()));
                    }
                    base + i
                }
                ArgRef::Name(n) => match names.iter().position(|x| x == n) {
                    Some(k) => base + npos + k,
                    None => match captures.iter().position(|x| x == n) {
                        Some(k) => base + npos + names.len() + k,
                        None => {
                            captures.push(n.clone());
                            srcs.push(Src::Capture(n.clone(), flo));
                            base + npos + names.len() + captures.len() - 1
                        }
                    },
                },
            })
        };
        let mut out = pieces;
        let mut used = vec![false; npos + names.len()];
        for p in parsed {
            match p {
                Piece::Lit(s) => out.push(Piece::Lit(s)),
                Piece::Ph(r, mut spec) => {
                    let fix = |cs: CountSpec, next: &mut usize, srcs: &mut Vec<Src>, resolve: &mut dyn FnMut(&ArgRef, &mut usize, &mut Vec<Src>) -> Result<usize, String>| -> Result<CountSpec, String> {
                        Ok(match cs {
                            CountSpec::Arg(a) => CountSpec::Arg(ArgRef::Index(resolve(&a, next, srcs)?)),
                            CountSpec::Star => CountSpec::Arg(ArgRef::Index(resolve(&ArgRef::Next, next, srcs)?)),
                            x => x,
                        })
                    };
                    spec.width = fix(std::mem::replace(&mut spec.width, CountSpec::Implied), &mut next, &mut srcs, &mut resolve).map_err(|m| PErr { pos: flo, msg: m })?;
                    spec.precision = fix(std::mem::replace(&mut spec.precision, CountSpec::Implied), &mut next, &mut srcs, &mut resolve).map_err(|m| PErr { pos: flo, msg: m })?;
                    let i = resolve(&r, &mut next, &mut srcs).map_err(|m| PErr { pos: flo, msg: m })?;
                    for cs in [&spec.width, &spec.precision] {
                        if let CountSpec::Arg(ArgRef::Index(k)) = cs {
                            if *k >= base && *k - base < used.len() {
                                used[*k - base] = true;
                            }
                        }
                    }
                    if i >= base && i - base < used.len() {
                        used[i - base] = true;
                    }
                    out.push(Piece::Ph(ArgRef::Index(i), spec));
                }
            }
        }
        if let Some(k) = used.iter().position(|u| !u) {
            let e = match srcs.get(base + k) {
                Some(Src::Expr(e)) => self.ast.expr(*e).lo,
                _ => flo,
            };
            return Err(PErr { pos: e, msg: "format argument never used -- remove it or add a `{}`".to_string() });
        }
        self.gen_pieces(g, out, srcs, flo)
    }

    /// Emits `Arguments::new_const / new_v1 / new_v1_formatted` for resolved pieces
    /// (`Ph` indices are slots into `srcs`).
    fn gen_pieces(&mut self, g: &mut Gen, pieces: Vec<Piece>, srcs: Vec<Src>, _lo: u32) -> PResult<()> {
        let (pieces, srcs) = self.inline_literals(pieces, srcs);
        // merge literals; pieces[i] precedes placeholder i
        let mut lits: Vec<String> = Vec::new();
        let mut phs: Vec<(usize, Spec)> = Vec::new();
        let mut cur = String::new();
        for p in pieces {
            match p {
                Piece::Lit(s) => cur.push_str(&s),
                Piece::Ph(ArgRef::Index(i), spec) => {
                    lits.push(std::mem::take(&mut cur));
                    phs.push((i, spec));
                }
                Piece::Ph(..) => {}
            }
        }
        if !cur.is_empty() {
            lits.push(cur);
        }
        let mut lit_text = String::from("&[");
        for (k, l) in lits.iter().enumerate() {
            if k > 0 {
                lit_text.push_str(", ");
            }
            lit_text.push_str(&str_lit(l));
        }
        lit_text.push(']');
        if phs.is_empty() {
            g.p(&format!("{}::fmt::Arguments::new_const({})", CORE, lit_text));
            return Ok(());
        }
        let formatted = phs.iter().any(|(_, s)| !is_default(s));
        // each slot evaluated once, in slot order: inline when the placeholders use every slot
        // exactly once in order and nothing else refers to them; otherwise bind them first
        let mut inline = !formatted && phs.len() == srcs.len();
        if inline {
            for (k, (i, _)) in phs.iter().enumerate() {
                if *i != k {
                    inline = false;
                }
            }
        }
        let slot = |g: &mut Gen, i: usize, inline: bool| {
            if inline {
                g.borrow(&srcs[i]);
            } else {
                g.p(&format!("__relax_a{}", i));
            }
        };
        if !inline {
            g.p("match (");
            for s in srcs.iter() {
                g.borrow(s);
                g.p(",");
            }
            g.p(") { (");
            for i in 0..srcs.len() {
                g.p(&format!("__relax_a{},", i));
            }
            g.p(") => ");
        }
        // args array: one entry per placeholder, then width/precision params
        let mut args: Vec<(usize, &'static str)> = Vec::new();
        for (i, s) in &phs {
            args.push((*i, s.ctor));
        }
        let count_pos = |cs: &CountSpec, args: &mut Vec<(usize, &'static str)>| -> String {
            match cs {
                CountSpec::Implied | CountSpec::Star => format!("{}::fmt::rt::Count::Implied", CORE),
                CountSpec::Is(n) => format!("{}::fmt::rt::Count::Is({})", CORE, n),
                CountSpec::Arg(ArgRef::Index(i)) => {
                    let k = match args.iter().position(|a| a.0 == *i && a.1 == "from_usize") {
                        Some(k) => k,
                        None => {
                            args.push((*i, "from_usize"));
                            args.len() - 1
                        }
                    };
                    format!("{}::fmt::rt::Count::Param({})", CORE, k)
                }
                CountSpec::Arg(_) => format!("{}::fmt::rt::Count::Implied", CORE),
            }
        };
        let mut specs = String::new();
        if formatted {
            specs.push_str("&[");
            for (k, (_, s)) in phs.iter().enumerate() {
                let prec = count_pos(&s.precision, &mut args);
                let width = count_pos(&s.width, &mut args);
                if k > 0 {
                    specs.push_str(", ");
                }
                specs.push_str(&format!("{}::fmt::rt::Placeholder::new({}, {:?}, {}, {}, {}, {})", CORE, k, s.fill, s.align, s.flags, prec, width));
            }
            specs.push(']');
        }
        if formatted {
            g.p(&format!("{}::fmt::Arguments::new_v1_formatted({}, &[", CORE, lit_text));
        } else {
            g.p(&format!("{}::fmt::Arguments::new_v1({}, &[", CORE, lit_text));
        }
        for (k, (i, ctor)) in args.iter().enumerate() {
            if k > 0 {
                g.p(", ");
            }
            g.p(&format!("{}::fmt::rt::Argument::{}(", CORE, ctor));
            slot(g, *i, inline);
            g.p(")");
        }
        g.p("]");
        if formatted {
            g.p(", ");
            g.p(&specs);
        }
        g.p(")");
        if !inline {
            g.p(" }");
        }
        Ok(())
    }

    /// rustc ast_lowering format.rs inline_literals: a `{}` (Display, no options) whose argument
    /// is a string literal or an integer literal that fits its type (unsuffixed: i32) becomes
    /// part of the literal pieces, and arguments no placeholder uses any more are dropped --
    /// `panic!("{}", "lit")` / `format_args!("x {}", 1)` then have `as_str()` and a
    /// `&'static str` panic payload.
    fn inline_literals(&mut self, pieces: Vec<Piece>, srcs: Vec<Src>) -> (Vec<Piece>, Vec<Src>) {
        let mut inlined = vec![false; srcs.len()];
        let mut out = Vec::with_capacity(pieces.len());
        for p in pieces {
            match p {
                Piece::Ph(ArgRef::Index(i), spec) if spec.ctor == "new_display" && is_default(&spec) && i < srcs.len() => match self.inline_lit_text(&srcs[i]) {
                    Some(t) => {
                        inlined[i] = true;
                        out.push(Piece::Lit(t));
                    }
                    None => out.push(Piece::Ph(ArgRef::Index(i), spec)),
                },
                p => out.push(p),
            }
        }
        if !inlined.iter().any(|x| *x) {
            return (out, srcs);
        }
        // slots still used by a placeholder (or a width / precision) stay
        let mut keep = vec![false; srcs.len()];
        for p in &out {
            if let Piece::Ph(ArgRef::Index(i), spec) = p {
                keep[*i] = true;
                for cs in [&spec.width, &spec.precision] {
                    if let CountSpec::Arg(ArgRef::Index(k)) = cs {
                        keep[*k] = true;
                    }
                }
            }
        }
        for i in 0..srcs.len() {
            if !inlined[i] {
                keep[i] = true;
            }
        }
        let mut map = vec![0usize; srcs.len()];
        let mut nsrcs = Vec::new();
        for (i, s) in srcs.into_iter().enumerate() {
            map[i] = nsrcs.len();
            if keep[i] {
                nsrcs.push(s);
            }
        }
        let remap = |cs: CountSpec| match cs {
            CountSpec::Arg(ArgRef::Index(k)) => CountSpec::Arg(ArgRef::Index(map[k])),
            x => x,
        };
        let out = out
            .into_iter()
            .map(|p| match p {
                Piece::Ph(ArgRef::Index(i), mut spec) => {
                    spec.width = remap(std::mem::replace(&mut spec.width, CountSpec::Implied));
                    spec.precision = remap(std::mem::replace(&mut spec.precision, CountSpec::Implied));
                    Piece::Ph(ArgRef::Index(map[i]), spec)
                }
                p => p,
            })
            .collect();
        (out, nsrcs)
    }

    /// The text a literal argument displays as, when rustc inlines it (try_inline_lit).
    fn inline_lit_text(&self, s: &Src) -> Option<String> {
        let mut e = match s {
            Src::Expr(e) => *e,
            _ => return None,
        };
        loop {
            match &self.ast.expr(e).kind {
                ExprKind::Paren(x) | ExprKind::AddrOf(false, false, x) => e = *x,
                _ => break,
            }
        }
        let ex = self.ast.expr(e);
        let kind = match &ex.kind {
            ExprKind::Lit(k) => k.clone(),
            _ => return None,
        };
        let text = String::from_utf8_lossy(self.bytes(ex.lo, ex.hi)).into_owned();
        match kind {
            LitKind::Str | LitKind::RawStr => Some(crate::lit::unescape_str(&text)),
            LitKind::Int => {
                let t = text.replace('_', "");
                let (radix, body) = if let Some(r) = t.strip_prefix("0x") {
                    (16, r)
                } else if let Some(r) = t.strip_prefix("0o") {
                    (8, r)
                } else if let Some(r) = t.strip_prefix("0b") {
                    (2, r)
                } else {
                    (10, t.as_str())
                };
                let sfx = ["i128", "u128", "isize", "usize", "i64", "u64", "i32", "u32", "i16", "u16", "i8", "u8"].iter().find(|x| body.ends_with(*x)).copied();
                // hex digits include `b`..`f` but no suffix starts with them
                let digits = match sfx {
                    Some(x) => &body[..body.len() - x.len()],
                    None => body,
                };
                let n = u128::from_str_radix(digits, radix).ok()?;
                let max: u128 = match sfx {
                    None | Some("i32") => i32::MAX as u128,
                    Some("i8") => i8::MAX as u128,
                    Some("i16") => i16::MAX as u128,
                    Some("i64") | Some("isize") => i64::MAX as u128,
                    Some("i128") => i128::MAX as u128,
                    Some("u8") => u8::MAX as u128,
                    Some("u16") => u16::MAX as u128,
                    Some("u32") => u32::MAX as u128,
                    Some("u64") | Some("usize") => u64::MAX as u128,
                    _ => u128::MAX,
                };
                if n > max {
                    return None;
                }
                Some(n.to_string())
            }
            _ => None,
        }
    }

    /// Appends generated text as an expansion of `name!` at `call_lo` and returns the index
    /// of its first token; `__relax_splice_N` idents become `T::Interp` tokens.
    pub(super) fn splice_expansion(&mut self, name: &str, g: &Gen, call_lo: u32) -> PResult<usize> {
        if let Some(t) = crate::preload::relax_trace() {
            if t.split(',').any(|p| p.strip_prefix("expand:").map_or(false, |n| name.contains(n))) {
                eprintln!("expand {}! => {}", name, g.s);
            }
        }
        let base = (self.src.len() + self.ext.len()) as u32;
        self.ext.extend_from_slice(g.s.as_bytes());
        let mut nt = Vec::new();
        crate::lexer::lex(g.s.as_bytes(), self.edition, &mut nt)
            .map_err(|e| PErr { pos: call_lo, msg: format!("internal error: `{}!` expansion does not lex: {}", name, e.msg) })?;
        let start = self.toks.len();
        for mut t in nt {
            if t.kind == T::Ident {
                let txt = &g.s.as_bytes()[t.lo as usize..t.hi as usize];
                if let Some(k) = txt.strip_prefix(SPLICE.as_bytes()) {
                    let k: usize = std::str::from_utf8(k).ok().and_then(|x| x.parse().ok()).unwrap_or(usize::MAX);
                    if let Some(&e) = g.splices.get(k) {
                        let x = self.ast.expr(e);
                        let (lo, hi) = (x.lo, x.hi);
                        self.interp.insert(self.toks.len() as u32, e);
                        self.toks.push(Tok { kind: T::Interp, lo, hi });
                        continue;
                    }
                }
            }
            t.lo += base;
            t.hi += base;
            self.toks.push(t);
        }
        if self.toks.last().map_or(true, |t| t.kind != T::Eof) {
            let e = base + g.s.len() as u32;
            self.toks.push(Tok { kind: T::Eof, lo: e, hi: e });
        }
        for (off, o, n) in &g.origins {
            self.tok_origin.push((base + off, *o, *n));
        }
        self.exp_map.push((base, base + g.s.len() as u32, call_lo));
        Ok(start)
    }
}

fn default_spec(ctor: &'static str) -> Spec {
    Spec { fill: ' ', align: 3, flags: 0, width: CountSpec::Implied, precision: CountSpec::Implied, ctor }
}

fn is_default(s: &Spec) -> bool {
    s.fill == ' ' && s.align == 3 && s.flags == 0 && matches!(s.width, CountSpec::Implied) && matches!(s.precision, CountSpec::Implied)
}

/// Parses a format string (already unescaped) into literal pieces and placeholders.
fn parse_format(s: &str) -> Result<Vec<Piece>, String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '{' if c.get(i + 1) == Some(&'{') => {
                lit.push('{');
                i += 2;
            }
            '}' if c.get(i + 1) == Some(&'}') => {
                lit.push('}');
                i += 2;
            }
            '}' => return Err("unmatched `}` in format string -- write `}}` for a literal brace".to_string()),
            '{' => {
                let mut j = i + 1;
                while j < c.len() && c[j] != '}' && c[j] != ':' {
                    j += 1;
                }
                let arg: String = c[i + 1..j.min(c.len())].iter().collect::<String>().trim().to_string();
                let mut spec = default_spec("new_display");
                if j < c.len() && c[j] == ':' {
                    let mut k = j + 1;
                    while k < c.len() && c[k] != '}' {
                        k += 1;
                    }
                    let sp: String = c[j + 1..k.min(c.len())].iter().collect();
                    spec = parse_spec(&sp)?;
                    j = k;
                }
                if j >= c.len() {
                    return Err("unterminated `{` in format string -- write `{{` for a literal brace".to_string());
                }
                if !lit.is_empty() {
                    out.push(Piece::Lit(std::mem::take(&mut lit)));
                }
                out.push(Piece::Ph(arg_ref(&arg)?, spec));
                i = j + 1;
            }
            ch => {
                lit.push(ch);
                i += 1;
            }
        }
    }
    if !lit.is_empty() {
        out.push(Piece::Lit(lit));
    }
    Ok(out)
}

fn arg_ref(a: &str) -> Result<ArgRef, String> {
    if a.is_empty() {
        return Ok(ArgRef::Next);
    }
    if let Ok(n) = a.parse::<usize>() {
        return Ok(ArgRef::Index(n));
    }
    let a = a.strip_prefix("r#").unwrap_or(a);
    let ok = a.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_') && a.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !ok {
        return Err(format!("invalid format argument `{}` -- use a name, an index or nothing", a));
    }
    Ok(ArgRef::Name(a.to_string()))
}

/// `[[fill]align][sign]['#']['0'][width]['.' precision][type]`
fn parse_spec(s: &str) -> Result<Spec, String> {
    let c: Vec<char> = s.chars().collect();
    let mut sp = default_spec("new_display");
    let mut i = 0;
    let align_of = |x: char| match x {
        '<' => Some(0u8),
        '>' => Some(1),
        '^' => Some(2),
        _ => None,
    };
    if c.len() >= 2 && align_of(c[1]).is_some() {
        sp.fill = c[0];
        sp.align = align_of(c[1]).unwrap_or(3);
        i = 2;
    } else if let Some(a) = c.first().and_then(|x| align_of(*x)) {
        sp.align = a;
        i = 1;
    }
    if c.get(i) == Some(&'+') {
        sp.flags |= 1;
        i += 1;
    } else if c.get(i) == Some(&'-') {
        sp.flags |= 2;
        i += 1;
    }
    if c.get(i) == Some(&'#') {
        sp.flags |= 4;
        i += 1;
    }
    // `0` flag, unless it is `0$` (argument 0 as width)
    if c.get(i) == Some(&'0') && c.get(i + 1) != Some(&'$') {
        sp.flags |= 8;
        i += 1;
    }
    let (w, ni) = count(&c, i)?;
    sp.width = w;
    i = ni;
    if c.get(i) == Some(&'.') {
        i += 1;
        if c.get(i) == Some(&'*') {
            sp.precision = CountSpec::Star;
            i += 1;
        } else {
            let (p, ni) = count(&c, i)?;
            if matches!(p, CountSpec::Implied) {
                return Err("missing precision after `.` in format spec".to_string());
            }
            sp.precision = p;
            i = ni;
        }
    }
    let ty: String = c[i..].iter().collect();
    sp.ctor = match ty.trim() {
        "" => "new_display",
        "?" => "new_debug",
        "x?" => {
            sp.flags |= 16;
            "new_debug"
        }
        "X?" => {
            sp.flags |= 32;
            "new_debug"
        }
        "x" => "new_lower_hex",
        "X" => "new_upper_hex",
        "o" => "new_octal",
        "b" => "new_binary",
        "e" => "new_lower_exp",
        "E" => "new_upper_exp",
        "p" => "new_pointer",
        t => return Err(format!("unknown format trait `{}` -- use one of ? x X o b e E p", t)),
    };
    Ok(sp)
}

/// A width/precision count at `i`: `N`, `N$`, `name$`, or nothing.
fn count(c: &[char], i: usize) -> Result<(CountSpec, usize), String> {
    let mut j = i;
    while j < c.len() && (c[j].is_alphanumeric() || c[j] == '_') {
        j += 1;
    }
    if j == i {
        return Ok((CountSpec::Implied, i));
    }
    let word: String = c[i..j].iter().collect();
    if c.get(j) == Some(&'$') {
        return Ok((CountSpec::Arg(arg_ref(&word)?), j + 1));
    }
    // a plain number; trailing letters belong to the type (`{:5x}`)
    let mut k = i;
    while k < c.len() && c[k].is_ascii_digit() {
        k += 1;
    }
    if k == i {
        return Ok((CountSpec::Implied, i));
    }
    let n: String = c[i..k].iter().collect();
    let n: usize = n.parse().map_err(|_| format!("format width `{}` too large", n))?;
    Ok((CountSpec::Is(n), k))
}
