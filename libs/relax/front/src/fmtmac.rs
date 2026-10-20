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

use crate::tg;
use crate::tokbuf::TokBuf;

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
/// tokens that already are a reference.
enum Src {
    Expr(ExprId),
    /// the local `name`, borrowed
    Local(&'static str),
    /// the call `name!()`, borrowed
    Call0(&'static str),
    /// `&&__relax_tmp`: already a reference
    RefTmp,
    /// an implicit capture `{name}`: `&name`, the name with the format string's position
    /// (hygiene: rustc resolves it in the format string's context)
    Capture(String, u32),
    /// generated tokens of a value (borrowed with `&`), e.g. nested `Arguments`
    Nested(TokBuf),
}

/// A width / precision of a placeholder.
enum Count {
    Implied,
    Is(usize),
    Param(usize),
}

impl Count {
    fn emit(&self, g: &mut Gen) {
        match self {
            Count::Implied => tg!(g; #core() :: fmt :: rt :: Count :: Implied),
            Count::Is(n) => tg!(g; #core() :: fmt :: rt :: Count :: Is (#int(*n))),
            Count::Param(k) => tg!(g; #core() :: fmt :: rt :: Count :: Param (#int(*k))),
        }
    }
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
    named_pos: Vec<u32>,
}

/// Generated tokens plus the expressions spliced into them.
pub(super) type Gen = TokBuf;

/// `&value` / the reference itself
fn borrow(g: &mut Gen, s: &Src) {
    match s {
        Src::Expr(e) => tg!(g; & #expr(*e)),
        Src::RefTmp => tg!(g; && __relax_tmp),
        Src::Capture(n, at) => tg!(g; & #word_at(n, *at)),
        Src::Local(t) => tg!(g; & (#word(t))),
        Src::Call0(t) => tg!(g; & (#word(t) ! ())),
        Src::Nested(b) => tg!(g; & (#append(b))),
    }
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
        // rustc_resolve: `#[no_implicit_prelude]` (edition 2018 and later) takes the std prelude's macros away
        if self.no_prelude_hides(path, &name) {
            return Err(PErr { pos: nlo, msg: format!("cannot find macro `{name}` in this scope") });
        }
        let save = self.pos;
        self.pos = toks.lo as usize;
        let r = self.builtin_gen(&name, toks.hi as usize, lo);
        self.pos = save;
        let g = r?;
        let start = self.splice_buf(&name, &g, lo)?;
        Ok(Some(self.in_expansion(start, |p| p.expr())?))
    }

    fn builtin_gen(&mut self, name: &str, hi: usize, lo: u32) -> PResult<Gen> {
        crate::preload::timed_named(3, name, || self.builtin_gen_inner(name, hi, lo))
    }

    fn builtin_gen_inner(&mut self, name: &str, hi: usize, lo: u32) -> PResult<Gen> {
        let mut g = Gen::new(self.edition);
        match name {
            "format_args" => {
                let c = self.fmt_call(hi, false)?;
                self.gen_args(&mut g, c, Vec::new(), Vec::new(), "")?;
            }
            "format" => {
                let c = self.fmt_call(hi, false)?;
                tg!(&mut g; #core() :: fmt :: format #open());
                self.gen_args(&mut g, c, Vec::new(), Vec::new(), "")?;
                tg!(&mut g; #close());
            }
            "print" | "println" | "eprint" | "eprintln" => {
                let c = self.fmt_call(hi, false)?;
                let f = if name.starts_with('e') { "_eprint" } else { "_print" };
                tg!(&mut g; #std() :: io :: #word(f) #open());
                self.gen_args(&mut g, c, Vec::new(), Vec::new(), if name.ends_with("ln") { "\n" } else { "" })?;
                tg!(&mut g; #close());
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
                tg!(&mut g; #expr(dst) . write_fmt #open());
                self.gen_args(&mut g, c, Vec::new(), Vec::new(), if name == "writeln" { "\n" } else { "" })?;
                tg!(&mut g; #close());
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                let c = self.fmt_call(hi, true)?;
                let prefix = match name {
                    "unreachable" => "internal error: entered unreachable code",
                    "todo" => "not yet implemented",
                    "unimplemented" => "not implemented",
                    _ => "",
                };
                tg!(&mut g; #core() :: panicking :: panic_at #open());
                let empty = c.fmt.is_none() && c.first.is_none();
                if empty {
                    let msg = if prefix.is_empty() { "explicit panic" } else { prefix };
                    tg!(&mut g; #core() :: fmt :: Arguments :: new_const (& [#str_lit(msg)]));
                } else if prefix.is_empty() {
                    self.gen_args(&mut g, c, Vec::new(), Vec::new(), "")?;
                } else {
                    // "<prefix>: <message>"
                    let mut inner = Gen::new(self.edition);
                    self.gen_args(&mut inner, c, Vec::new(), Vec::new(), "")?;
                    let pieces = vec![Piece::Lit(format!("{}: ", prefix)), Piece::Ph(ArgRef::Index(0), default_spec("new_display"))];
                    self.gen_pieces(&mut g, pieces, vec![Src::Nested(inner)], lo)?;
                }
                tg!(&mut g; , #caller_location() #close());
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
                tg!(&mut g; if ! (#expr(cond)) #open_brace() #core() :: panicking :: panic_at #open());
                if c.fmt.is_none() {
                    tg!(&mut g; #core() :: fmt :: Arguments :: new_const (& [#str_lit(&format!("assertion failed: {}", text))]));
                } else {
                    self.gen_args(&mut g, c, Vec::new(), Vec::new(), "")?;
                }
                tg!(&mut g; , #caller_location() #close() #close_brace());
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
                // match (&a, &b) { (l, r) => { if <cond> { panic_at(..) } } }, closed below
                tg!(&mut g; match (& #expr(a), & #expr(b)) #open_brace() (__relax_l, __relax_r) => #open_brace());
                if eq {
                    tg!(&mut g; if ! (* __relax_l == * __relax_r));
                } else {
                    tg!(&mut g; if * __relax_l == * __relax_r);
                }
                g.punct(T::OpenBrace);
                tg!(&mut g; #core() :: panicking :: panic_at #open());
                let op = if eq { "==" } else { "!=" };
                // rustc's assert_failed formats `&left` (a `&&T`, `T: ?Sized`): `&T: Debug`
                // keeps unsized operands (`dyn Tr`, `[T]`) out of the Sized-bounded Argument::new_debug
                let l = Src::Local("__relax_l");
                let r = Src::Local("__relax_r");
                let dbg = || default_spec("new_debug");
                let mut pieces = vec![Piece::Lit(format!("assertion `left {} right` failed", op))];
                let mut srcs = Vec::new();
                if c.fmt.is_some() || c.first.is_some() {
                    let mut inner = Gen::new(self.edition);
                    self.gen_args(&mut inner, c, Vec::new(), Vec::new(), "")?;
                    pieces.push(Piece::Lit(": ".to_string()));
                    pieces.push(Piece::Ph(ArgRef::Index(0), default_spec("new_display")));
                    srcs.push(Src::Nested(inner));
                }
                let n = srcs.len();
                pieces.push(Piece::Lit("\n  left: ".to_string()));
                pieces.push(Piece::Ph(ArgRef::Index(n), dbg()));
                pieces.push(Piece::Lit("\n right: ".to_string()));
                pieces.push(Piece::Ph(ArgRef::Index(n + 1), dbg()));
                srcs.push(l);
                srcs.push(r);
                self.gen_pieces(&mut g, pieces, srcs, lo)?;
                tg!(&mut g; , #caller_location() #close() #close_brace() #close_brace() #close_brace());
            }
            "vec" => {
                if self.pos >= hi {
                    tg!(&mut g; #core() :: vec :: Vec :: new ());
                } else {
                    let first = self.expr()?;
                    if self.eat(T::Semi) {
                        let n = self.expr()?;
                        tg!(&mut g; #core() :: vec :: from_elem (#expr(first), #expr(n)));
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
                        tg!(&mut g; < [_] > :: into_vec #open() #core() :: boxed :: Box :: new #open() #open_bracket());
                        for e in v {
                            tg!(&mut g; #expr(e) ,);
                        }
                        tg!(&mut g; #close_bracket() #close() #close());
                    }
                }
            }
            "dbg" => {
                // dbg!(e) -> match e { tmp => { eprint!("[file:line:col] text = {:#?}\n", &tmp); tmp } }
                if self.pos >= hi {
                    tg!(&mut g; #std() :: io :: _eprint #open());
                    let pieces = vec![
                        Piece::Lit("[".to_string()),
                        Piece::Ph(ArgRef::Index(0), default_spec("new_display")),
                        Piece::Lit(":".to_string()),
                        Piece::Ph(ArgRef::Index(1), default_spec("new_display")),
                        Piece::Lit(":".to_string()),
                        Piece::Ph(ArgRef::Index(2), default_spec("new_display")),
                        Piece::Lit("]\n".to_string()),
                    ];
                    let srcs = vec![Src::Call0("file"), Src::Call0("line"), Src::Call0("column")];
                    self.gen_pieces(&mut g, pieces, srcs, lo)?;
                    tg!(&mut g; #close());
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
                            g.punct(T::OpenParen);
                        }
                    } else {
                        g.punct(T::Comma);
                    }
                    first = false;
                    tg!(&mut g; match #expr(e) #open_brace() __relax_tmp => #open_brace());
                    tg!(&mut g; #std() :: io :: _eprint #open());
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
                    let srcs = vec![Src::Call0("file"), Src::Call0("line"), Src::Call0("column"), Src::RefTmp];
                    self.gen_pieces(&mut g, pieces, srcs, lo)?;
                    tg!(&mut g; #close() ; __relax_tmp #close_brace() #close_brace());
                    if !multi {
                        break;
                    }
                    }
                    if multi {
                        tg!(&mut g; , #close());
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
    /// The format string a macro call at the cursor expands to (rustc_builtin_macros format.rs
    /// `expr_to_spanned_string`: the argument is expanded eagerly, through `macro_rules!` calls
    /// that expand to further calls, down to a string literal); None when it is not one.
    fn fmt_macro_literal(&mut self, hi: usize) -> PResult<Option<(String, usize)>> {
        if self.macros.is_some() && self.mac_ahead() {
            let save = self.pos;
            let lo = self.lo();
            let path = self.path(true)?;
            if let Some(def) = self.rules_macro(&path) {
                self.expect(T::Not)?;
                let (_, toks) = self.delim_tt()?;
                let end = self.pos;
                if end == hi || self.toks.at(end).kind == T::Comma {
                    let start = self.expand_call(&def, toks, lo)?;
                    let eof = (start..).find(|&i| self.toks.at(i).kind == T::Eof).unwrap_or(start);
                    let inner = self.in_expansion(start, |p| {
                        if matches!(p.kind(), T::Str | T::RawStr) && p.pos + 1 == eof {
                            let t = p.tok();
                            let text = String::from_utf8_lossy(p.tok_text(t)).into_owned();
                            p.pos += 1;
                            return Ok(Some(crate::lit::unescape_str(&text)));
                        }
                        let inner = p.fmt_macro_literal(eof)?;
                        if let Some((_, end)) = &inner {
                            p.pos = *end;
                        }
                        Ok(inner.map(|(text, _)| text))
                    })?;
                    self.pos = end;
                    return Ok(inner.map(|text| (text, end)));
                }
            }
            self.pos = save;
        }
        Ok(self.fmt_builtin_literal(hi))
    }

    fn fmt_builtin_literal(&self, hi: usize) -> Option<(String, usize)> {
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
        let mut c = FmtCall { fmt: None, first: None, pos: Vec::new(), named: Vec::new(), named_pos: Vec::new() };
        if self.pos >= hi {
            return Ok(c);
        }
        let t = self.tok();
        let lit_alone = matches!(t.kind, T::Str | T::RawStr) && (self.pos + 1 == hi || self.nth(1) == T::Comma);
        if lit_alone {
            self.pos += 1;
            let text = String::from_utf8_lossy(self.tok_text(t)).into_owned();
            c.fmt = Some((crate::lit::unescape_str(&text), t.lo));
        } else if let Some((text, end)) = self.fmt_macro_literal(hi)? {
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
                c.named_pos.push(n.lo);
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
        // named arguments reached by position (named_arguments_used_positionally)
        let mut numeric_named: Vec<usize> = Vec::new();
        let mut resolve = |r: &ArgRef, next: &mut usize, srcs: &mut Vec<Src>| -> Result<usize, String> {
            Ok(match r {
                ArgRef::Next => {
                    let i = *next;
                    *next += 1;
                    if i >= npos + names.len() {
                        return Err(format!("format string needs argument {} but there are {} -- add the argument", i + 1, npos + names.len()));
                    }
                    if i >= npos { numeric_named.push(i - npos); }
                    base + i
                }
                ArgRef::Index(i) => {
                    if *i >= npos + names.len() {
                        return Err(format!("format string refers to argument {} but there are {} -- add the argument", i, npos + names.len()));
                    }
                    if *i >= npos { numeric_named.push(*i - npos); }
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
        if crate::macros::uses::recording() {
            let file = self.bctx.as_ref().map_or("", |c| c.path.as_str()).to_string();
            for k in numeric_named {
                crate::macros::uses::named_arg_positional(&file, c.named_pos[k], &names[k]);
            }
        }
        self.gen_pieces(g, out, srcs, flo)
    }

    /// Whether the parsed expression `e` has an `.await` or a `yield` in it (a suspension point).
    fn expr_may_yield(&self, e: ExprId) -> bool {
        let ex = self.ast.expr(e);
        let (lo, hi) = (ex.lo, ex.hi);
        let n = self.toks.len();
        let (mut a, mut b) = (0, n);
        while a < b {
            let mid = (a + b) / 2;
            if self.toks.at(mid).lo < lo { a = mid + 1 } else { b = mid }
        }
        let mut i = a;
        while i < n {
            let t = self.toks.at(i);
            if t.lo >= hi { break; }
            if matches!(t.kind, T::KwAwait | T::KwYield) { return true; }
            i += 1;
        }
        false
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
        // `&[lit, lit, ..]`
        let emit_lits = |g: &mut Gen| {
            tg!(g; & #open_bracket());
            for (k, l) in lits.iter().enumerate() {
                if k > 0 {
                    g.punct(T::Comma);
                }
                g.str_lit(l);
            }
            g.punct(T::CloseBracket);
        };
        if phs.is_empty() {
            tg!(g; #core() :: fmt :: Arguments :: new_const #open());
            emit_lits(g);
            g.close();
            return Ok(());
        }
        let formatted = phs.iter().any(|(_, s)| !is_default(s));
        // each slot evaluated once, in slot order: inline when the placeholders use every slot
        // exactly once in order and nothing else refers to them; otherwise bind them first
        let mut inline = !formatted && phs.len() == srcs.len();
        // rustc ast_lowering format.rs: the arguments of the later slots are evaluated before any `Argument` is
        // made (`super let args = (&arg0, &arg1)`), so none of them is held across a suspension point in them
        if inline && srcs.iter().skip(1).any(|s| matches!(s, Src::Expr(e) if self.expr_may_yield(*e))) {
            inline = false;
        }
        if inline {
            for (k, (i, _)) in phs.iter().enumerate() {
                if *i != k {
                    inline = false;
                }
            }
        }
        // args array: one entry per placeholder, then width/precision params
        let mut args: Vec<(usize, &'static str)> = Vec::new();
        for (i, s) in &phs {
            args.push((*i, s.ctor));
        }
        let count_pos = |cs: &CountSpec, args: &mut Vec<(usize, &'static str)>| -> Count {
            match cs {
                CountSpec::Implied | CountSpec::Star => Count::Implied,
                CountSpec::Is(n) => Count::Is(*n),
                CountSpec::Arg(ArgRef::Index(i)) => {
                    let k = match args.iter().position(|a| a.0 == *i && a.1 == "from_usize") {
                        Some(k) => k,
                        None => {
                            args.push((*i, "from_usize"));
                            args.len() - 1
                        }
                    };
                    Count::Param(k)
                }
                CountSpec::Arg(_) => Count::Implied,
            }
        };
        // (placeholder index, spec, precision, width)
        let mut specs: Vec<(usize, &Spec, Count, Count)> = Vec::new();
        if formatted {
            for (k, (_, s)) in phs.iter().enumerate() {
                let prec = count_pos(&s.precision, &mut args);
                let width = count_pos(&s.width, &mut args);
                specs.push((k, s, prec, width));
            }
        }
        if formatted {
            tg!(g; #core() :: fmt :: Arguments :: new_v1_formatted #open());
        } else {
            tg!(g; #core() :: fmt :: Arguments :: new_v1 #open());
        }
        emit_lits(g);
        g.punct(T::Comma);
        // rustc (pre-`super let`) lowering: the arguments array is `&[..]` when the placeholders use
        // every slot once in order, else `&match (&a0, &a1) { (a0, a1) => [..] }`; either way the
        // array temporary belongs to the enclosing expression, not to a match arm.
        if inline {
            tg!(g; & #open_bracket());
        } else {
            tg!(g; & match #open());
            for s in srcs.iter() {
                borrow(g, s);
                g.punct(T::Comma);
            }
            tg!(g; #close() #open_brace() #open());
            for i in 0..srcs.len() {
                g.word(&format!("__relax_a{}", i));
                g.punct(T::Comma);
            }
            tg!(g; #close() => #open_bracket());
        }
        for (k, (i, ctor)) in args.iter().enumerate() {
            if k > 0 {
                g.punct(T::Comma);
            }
            tg!(g; #core() :: fmt :: rt :: Argument :: #word(ctor) #open());
            if inline {
                borrow(g, &srcs[*i]);
            } else {
                g.word(&format!("__relax_a{}", i));
            }
            g.close();
        }
        g.punct(T::CloseBracket);
        if !inline {
            g.punct(T::CloseBrace);
        }
        if formatted {
            tg!(g; , & #open_bracket());
            for (n, (k, s, prec, width)) in specs.iter().enumerate() {
                if n > 0 {
                    g.punct(T::Comma);
                }
                tg!(g; #core() :: fmt :: rt :: Placeholder :: new #open() #int(*k) , #char_lit(s.fill) , #int(s.align) , #int(s.flags) ,);
                prec.emit(g);
                g.punct(T::Comma);
                width.emit(g);
                g.close();
            }
            g.punct(T::CloseBracket);
        }
        g.close();
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

    /// Appends generated tokens as an expansion of `name!` at `call_lo` and returns the index
    /// of its first token; a spliced expression becomes a `T::Interp` token.
    pub(super) fn splice_buf(&mut self, name: &str, g: &Gen, call_lo: u32) -> PResult<usize> {
        if let Some(t) = crate::preload::relax_trace() {
            if t.split(',').any(|p| p.strip_prefix("expand:").map_or(false, |n| name.contains(n))) {
                eprintln!("expand {}! => {}", name, g.s);
            }
        }
        if super::verify_tokens() {
            g.verify(name);
        }
        let base = (self.src.len() + self.ext.len()) as u32;
        self.ext.extend_from_slice(g.s.as_bytes());
        let start = self.toks.len();
        let mut interps = g.interps.iter().peekable();
        for (i, t) in g.toks.iter().enumerate() {
            if let Some(&&(at, e)) = interps.peek() {
                if at as usize == i {
                    interps.next();
                    let x = self.ast.expr(e);
                    let (lo, hi) = (x.lo, x.hi);
                    self.interp.insert(self.toks.len() as u32, e);
                    self.toks.push(Tok { kind: T::Interp, lo, hi });
                    continue;
                }
            }
            self.toks.push(Tok { kind: t.kind, lo: t.lo + base, hi: t.hi + base });
        }
        let e = base + g.s.len() as u32;
        self.toks.push(Tok { kind: T::Eof, lo: e, hi: e });
        for (off, o, n) in &g.origins {
            self.tok_origin.push((base + off, *o, *n));
        }
        self.exp_map.push((base, e, call_lo));
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
                    // rustc_parse_format `format`: a character followed by an alignment is the fill, even `}`
                    if matches!(c.get(k + 1), Some('<' | '>' | '^')) {
                        k += 2;
                    }
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
