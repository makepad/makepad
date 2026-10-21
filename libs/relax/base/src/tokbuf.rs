//! Generated code as tokens: the builder every built-in expansion writes (format macros, text
//! built-ins, builtin derives). Each token is pushed with its kind over a backing text (the
//! bytes the spans point into, like `Transcript` for `macro_rules!`); nothing is ever lexed.
//! `MAKEPAD_RUN_VERIFY_TOKENS=1` lexes the text of every buffer and compares it with the tokens.
//!
//! Templates are written with `tg!`: Rust tokens are the generated tokens, `#method(args)`
//! calls a builder method for the dynamic parts.
//!
//! ```ignore
//! tg!(g; fn #word(&name) ( & self ) -> #core() :: fmt :: Result { #body(x) });
//! ```

use crate::ast::ExprId;
use crate::lexer::{Tok, T};

/// Tokens over their text.
#[derive(Clone, Default)]
pub struct TokBuf {
    edition: u16,
    pub s: String,
    /// positions relative to `s`; a spliced expression's slot is an identifier placeholder
    pub toks: Vec<Tok>,
    /// (token index, expression) of the spliced expressions (`T::Interp` once spliced)
    pub interps: Vec<(u32, ExprId)>,
    /// generated text standing for source tokens: (offset, origin position, length)
    pub origins: Vec<(u32, u32, u32)>,
}

/// A verification failure: a panic in a macro worker is swallowed and hangs the run, so say it and stop.
pub fn verify_fail(msg: &str) -> ! {
    eprintln!("MAKEPAD_RUN_VERIFY_TOKENS: {}", msg);
    std::process::abort();
}

/// Paths in generated code: crates by identity (resolve from any crate, including core itself).
pub const CORE: &str = "__relax_crate_core";
pub const STD: &str = "__relax_crate_std";
/// The identifier a spliced expression stands for in the backing text.
pub const SPLICE: &str = "__relax_splice_";

pub fn punct_text(k: T) -> &'static str {
    match k {
        T::OpenParen => "(",
        T::CloseParen => ")",
        T::OpenBracket => "[",
        T::CloseBracket => "]",
        T::OpenBrace => "{",
        T::CloseBrace => "}",
        T::Semi => ";",
        T::Comma => ",",
        T::Dot => ".",
        T::DotDot => "..",
        T::DotDotDot => "...",
        T::DotDotEq => "..=",
        T::Colon => ":",
        T::PathSep => "::",
        T::RArrow => "->",
        T::FatArrow => "=>",
        T::Pound => "#",
        T::Question => "?",
        T::At => "@",
        T::Eq => "=",
        T::EqEq => "==",
        T::Ne => "!=",
        T::Lt => "<",
        T::Le => "<=",
        T::Gt => ">",
        T::Ge => ">=",
        T::Shl => "<<",
        T::Shr => ">>",
        T::Plus => "+",
        T::Minus => "-",
        T::Star => "*",
        T::Slash => "/",
        T::Percent => "%",
        T::Caret => "^",
        T::Not => "!",
        T::And => "&",
        T::Or => "|",
        T::AndAnd => "&&",
        T::OrOr => "||",
        T::PlusEq => "+=",
        T::MinusEq => "-=",
        _ => panic!("tokbuf: no fixed spelling for {:?}", k),
    }
}

impl TokBuf {
    pub fn new(edition: u16) -> TokBuf {
        TokBuf { edition, ..Default::default() }
    }

    /// Tokens already built over their text (positions relative to it; a cached expansion).
    pub fn from_parts(edition: u16, s: String, toks: Vec<Tok>) -> TokBuf {
        TokBuf { edition, s, toks, ..Default::default() }
    }

    /// Drops a final `,`.
    pub fn trim_comma(&mut self) {
        if let Some(t) = self.toks.last().copied() {
            if t.kind == T::Comma {
                self.toks.pop();
                self.s.truncate(t.lo as usize);
            }
        }
    }

    pub fn edition(&self) -> u16 {
        self.edition
    }

    pub fn is_empty(&self) -> bool {
        self.toks.is_empty()
    }

    /// One token of `kind` spelled `text` (one separating space before it, so an expansion's
    /// first token never starts at the previous expansion's end).
    pub fn put(&mut self, kind: T, text: &str) {
        self.s.push(' ');
        let lo = self.s.len() as u32;
        self.s.push_str(text);
        self.toks.push(Tok { kind, lo, hi: lo + text.len() as u32 });
    }

    /// A punctuation or delimiter token (its spelling is fixed by the kind).
    pub fn punct(&mut self, kind: T) {
        self.put(kind, punct_text(kind));
    }

    /// An identifier or keyword, classified as the lexer would for this edition; also a raw
    /// identifier (`r#type`), a lifetime (`'a`) and a tuple field index (`0`), as the source
    /// spelled a name.
    pub fn word(&mut self, text: &str) {
        let b = text.as_bytes();
        let kind = match b.first() {
            Some(b'\'') => T::Lifetime,
            Some(c) if c.is_ascii_digit() => T::Int,
            _ if text.starts_with("r#") => T::RawIdent,
            _ => crate::lexer::keyword(b, self.edition).unwrap_or(T::Ident),
        };
        self.put(kind, text);
    }

    /// A string literal token for `value` (Rust `Debug` escapes).
    pub fn str_lit(&mut self, value: &str) {
        self.put(T::Str, &format!("{:?}", value));
    }

    /// A char literal token for `c`.
    pub fn char_lit(&mut self, c: char) {
        self.put(T::Char, &format!("{:?}", c));
    }

    /// An integer literal token (digits, maybe with a type suffix: `7u32`).
    pub fn int<N: std::fmt::Display>(&mut self, n: N) {
        self.put(T::Int, &n.to_string());
    }

    /// A literal written in a template (`stringify!` of a Rust literal).
    pub fn lit(&mut self, text: &str) {
        let b = text.as_bytes();
        let kind = match b[0] {
            _ if text == "true" => T::KwTrue,
            _ if text == "false" => T::KwFalse,
            b'"' => T::Str,
            b'\'' => T::Char,
            b'b' if b.get(1) == Some(&b'"') => T::ByteStr,
            b'b' if b.get(1) == Some(&b'\'') => T::Byte,
            _ if text.contains('.') && !text.starts_with("0x") => T::Float,
            _ => T::Int,
        };
        self.put(kind, text);
    }

    /// `::__relax_crate_core` (the path prefix of core items in generated code).
    pub fn core(&mut self) {
        self.punct(T::PathSep);
        self.word(CORE);
    }

    /// `::__relax_crate_std`.
    pub fn std(&mut self) {
        self.punct(T::PathSep);
        self.word(STD);
    }

    /// `compile_error!("msg")`, with a closing `;` for an item.
    pub fn compile_error(edition: u16, msg: &str, item: bool) -> TokBuf {
        let mut g = TokBuf::new(edition);
        crate::tg!(&mut g; compile_error ! (#str_lit(msg)));
        if item {
            g.punct(T::Semi);
        }
        g
    }

    pub fn open(&mut self) {
        self.punct(T::OpenParen);
    }
    pub fn close(&mut self) {
        self.punct(T::CloseParen);
    }
    pub fn open_brace(&mut self) {
        self.punct(T::OpenBrace);
    }
    pub fn close_brace(&mut self) {
        self.punct(T::CloseBrace);
    }
    pub fn open_bracket(&mut self) {
        self.punct(T::OpenBracket);
    }
    pub fn close_bracket(&mut self) {
        self.punct(T::CloseBracket);
    }

    /// `::core::panic::Location::caller()` (the caller's location inside a #[track_caller] fn,
    /// else this site; rustc panic! uses Location::caller()).
    pub fn caller_location(&mut self) {
        self.core();
        self.punct(T::PathSep);
        self.word("panic");
        self.punct(T::PathSep);
        self.word("Location");
        self.punct(T::PathSep);
        self.word("caller");
        self.open();
        self.close();
    }

    /// A spliced, already parsed expression.
    pub fn expr(&mut self, e: ExprId) {
        let n = self.interps.len();
        self.interps.push((self.toks.len() as u32, e));
        let name = format!("{}{}", SPLICE, n);
        self.put(T::Ident, &name);
    }

    /// An identifier that stands for the source token at `origin` (hygiene: it resolves in the
    /// context of that position).
    pub fn word_at(&mut self, text: &str, origin: u32) {
        self.word(text);
        let t = self.toks[self.toks.len() - 1];
        self.origins.push((t.lo, origin, text.len() as u32));
    }

    /// Appends another buffer's tokens (and spliced expressions).
    pub fn append(&mut self, other: &TokBuf) {
        self.s.push(' ');
        let base = self.s.len() as u32;
        let first = self.toks.len() as u32;
        self.s.push_str(&other.s);
        self.toks.extend(other.toks.iter().map(|t| Tok { kind: t.kind, lo: t.lo + base, hi: t.hi + base }));
        self.origins.extend(other.origins.iter().map(|&(o, p, n)| (o + base, p, n)));
        for &(i, e) in &other.interps {
            self.interps.push((first + i, e));
        }
    }

    /// Debug check: lexing the text gives these tokens.
    pub fn verify(&self, what: &str) {
        let mut lexed = Vec::new();
        let ok = crate::lexer::lex(self.s.as_bytes(), self.edition, &mut lexed).is_ok();
        lexed.retain(|t| t.kind != T::Eof);
        let same = ok && lexed.len() == self.toks.len() && lexed.iter().zip(&self.toks).all(|(a, b)| a.lo == b.lo && a.hi == b.hi && a.kind == b.kind);
        if !same {
            let at = lexed.iter().zip(&self.toks).position(|(a, b)| !(a.lo == b.lo && a.hi == b.hi && a.kind == b.kind)).unwrap_or(lexed.len().min(self.toks.len()));
            let near = |t: Option<&Tok>| t.map(|t| (t.kind, t.lo, t.hi));
            // (a panic in a macro worker is swallowed and hangs the run: say it and stop)
            eprintln!("MAKEPAD_RUN_VERIFY_TOKENS: generated tokens of `{}` differ from the lexed text at token {} (lexed {} tokens, generated {}): lexed {:?} generated {:?} in `{}`", what, at, lexed.len(), self.toks.len(), near(lexed.get(at)), near(self.toks.get(at)), self.s);
            std::process::abort();
        }
    }
}

/// Writes tokens into a `TokBuf`: `tg!(g; <template tokens>)`. Rust identifiers and keywords
/// are the generated ones, `#name(args)` calls `g.name(args)`, groups nest. `#[..]` is an attribute.
#[macro_export]
macro_rules! tg {
    ($g:expr; $($t:tt)*) => {{
        let g: &mut $crate::tokbuf::TokBuf = $g;
        $crate::tg!(@ g; $($t)*);
    }};
    (@ $g:ident;) => {};
    (@ $g:ident; # [ $($i:tt)* ] $($r:tt)*) => {
        $g.punct($crate::lexer::T::Pound);
        $g.punct($crate::lexer::T::OpenBracket);
        $crate::tg!(@ $g; $($i)*);
        $g.punct($crate::lexer::T::CloseBracket);
        $crate::tg!(@ $g; $($r)*);
    };
    (@ $g:ident; # $f:ident ( $($a:tt)* ) $($r:tt)*) => {
        $g.$f($($a)*);
        $crate::tg!(@ $g; $($r)*);
    };
    (@ $g:ident; ( $($i:tt)* ) $($r:tt)*) => {
        $g.punct($crate::lexer::T::OpenParen);
        $crate::tg!(@ $g; $($i)*);
        $g.punct($crate::lexer::T::CloseParen);
        $crate::tg!(@ $g; $($r)*);
    };
    (@ $g:ident; [ $($i:tt)* ] $($r:tt)*) => {
        $g.punct($crate::lexer::T::OpenBracket);
        $crate::tg!(@ $g; $($i)*);
        $g.punct($crate::lexer::T::CloseBracket);
        $crate::tg!(@ $g; $($r)*);
    };
    (@ $g:ident; { $($i:tt)* } $($r:tt)*) => {
        $g.punct($crate::lexer::T::OpenBrace);
        $crate::tg!(@ $g; $($i)*);
        $g.punct($crate::lexer::T::CloseBrace);
        $crate::tg!(@ $g; $($r)*);
    };
    (@ $g:ident; ; $($r:tt)*) => { $g.punct($crate::lexer::T::Semi); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; , $($r:tt)*) => { $g.punct($crate::lexer::T::Comma); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; :: $($r:tt)*) => { $g.punct($crate::lexer::T::PathSep); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; : $($r:tt)*) => { $g.punct($crate::lexer::T::Colon); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; -> $($r:tt)*) => { $g.punct($crate::lexer::T::RArrow); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; => $($r:tt)*) => { $g.punct($crate::lexer::T::FatArrow); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; == $($r:tt)*) => { $g.punct($crate::lexer::T::EqEq); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; != $($r:tt)*) => { $g.punct($crate::lexer::T::Ne); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; += $($r:tt)*) => { $g.punct($crate::lexer::T::PlusEq); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; -= $($r:tt)*) => { $g.punct($crate::lexer::T::MinusEq); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; ^ $($r:tt)*) => { $g.punct($crate::lexer::T::Caret); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; - $($r:tt)*) => { $g.punct($crate::lexer::T::Minus); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; / $($r:tt)*) => { $g.punct($crate::lexer::T::Slash); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; % $($r:tt)*) => { $g.punct($crate::lexer::T::Percent); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; <= $($r:tt)*) => { $g.punct($crate::lexer::T::Le); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; >= $($r:tt)*) => { $g.punct($crate::lexer::T::Ge); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; << $($r:tt)*) => { $g.punct($crate::lexer::T::Shl); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; >> $($r:tt)*) => { $g.punct($crate::lexer::T::Shr); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; || $($r:tt)*) => { $g.punct($crate::lexer::T::OrOr); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; .. $($r:tt)*) => { $g.punct($crate::lexer::T::DotDot); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; = $($r:tt)*) => { $g.punct($crate::lexer::T::Eq); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; && $($r:tt)*) => { $g.punct($crate::lexer::T::AndAnd); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; & $($r:tt)*) => { $g.punct($crate::lexer::T::And); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; < $($r:tt)*) => { $g.punct($crate::lexer::T::Lt); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; > $($r:tt)*) => { $g.punct($crate::lexer::T::Gt); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; + $($r:tt)*) => { $g.punct($crate::lexer::T::Plus); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; * $($r:tt)*) => { $g.punct($crate::lexer::T::Star); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; ! $($r:tt)*) => { $g.punct($crate::lexer::T::Not); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; . $($r:tt)*) => { $g.punct($crate::lexer::T::Dot); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; ? $($r:tt)*) => { $g.punct($crate::lexer::T::Question); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; | $($r:tt)*) => { $g.punct($crate::lexer::T::Or); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; _ $($r:tt)*) => { $g.word("_"); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; $l:lifetime $($r:tt)*) => { $g.put($crate::lexer::T::Lifetime, stringify!($l)); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; $l:literal $($r:tt)*) => { $g.lit(stringify!($l)); $crate::tg!(@ $g; $($r)*); };
    (@ $g:ident; $i:ident $($r:tt)*) => { $g.word(stringify!($i)); $crate::tg!(@ $g; $($r)*); };
}
