//! Source-only Elixir lexer shared by the editor and structural frontend.
//! Tokens never cross a newline; literal continuation belongs to the provider.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

pub const ELIXIR_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum Mode {
    #[default]
    Normal,
    Literal(Literal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Literal {
    kind: ElixirKind,
    open: u8,
    close: u8,
    depth: usize,
    fence: bool,
    interpolate: bool,
    braces: usize,
    inner_quote: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ElixirState {
    mode: Mode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ElixirKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Alias,
    Atom,
    KeywordKey,
    Keyword,
    Attribute,
    Number,
    Char,
    String,
    Charlist,
    Heredoc,
    Sigil,
    Operator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElixirToken {
    pub start: u32,
    pub end: u32,
    pub kind: ElixirKind,
}

impl ElixirKind {
    pub fn role(self) -> TokenRole {
        match self {
            Self::Whitespace | Self::Newline => TokenRole::Whitespace,
            Self::Comment => TokenRole::Comment,
            Self::Identifier => TokenRole::Identifier,
            Self::Alias => TokenRole::Typename,
            Self::Atom | Self::KeywordKey => TokenRole::Constant,
            Self::Keyword => TokenRole::Keyword,
            Self::Attribute => TokenRole::Macro,
            Self::Number => TokenRole::Number,
            Self::Char => TokenRole::Char,
            Self::String | Self::Charlist | Self::Heredoc | Self::Sigil => TokenRole::String,
            Self::Operator => TokenRole::Punctuator,
            Self::Delimiter => TokenRole::Delimiter,
            Self::Unknown => TokenRole::Unknown,
        }
    }
}

fn is_keyword(s: &[u8]) -> bool {
    matches!(
        s,
        b"true"
            | b"false"
            | b"nil"
            | b"when"
            | b"and"
            | b"or"
            | b"not"
            | b"in"
            | b"fn"
            | b"do"
            | b"end"
            | b"catch"
            | b"rescue"
            | b"after"
            | b"else"
            | b"def"
            | b"defp"
            | b"defmacro"
            | b"defmacrop"
            | b"defguard"
            | b"defguardp"
            | b"defdelegate"
            | b"defmodule"
            | b"defprotocol"
            | b"defimpl"
            | b"defstruct"
            | b"defexception"
            | b"defoverridable"
            | b"import"
            | b"alias"
            | b"require"
            | b"use"
            | b"quote"
            | b"unquote"
            | b"unquote_splicing"
            | b"case"
            | b"cond"
            | b"if"
            | b"unless"
            | b"with"
            | b"for"
            | b"receive"
            | b"try"
            | b"super"
            | b"__MODULE__"
            | b"__DIR__"
            | b"__ENV__"
            | b"__CALLER__"
            | b"__STACKTRACE__"
    )
}

fn newline(b: u8) -> bool {
    matches!(b, b'\r' | b'\n')
}

fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn ident_end(bytes: &[u8], mut i: usize, suffix: bool) -> usize {
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        i += 1;
    }
    if suffix && matches!(bytes.get(i), Some(b'?' | b'!')) {
        i += 1;
    }
    i
}

fn key_suffix(bytes: &[u8], i: usize) -> bool {
    bytes.get(i) == Some(&b':') && bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace())
}

// Longest first, including bitstring delimiters (classified separately).
const OPERATORS: &[&[u8]] = &[
    b"..//", b"|||", b"&&&", b"<<<", b">>>", b"<<~", b"~>>", b"<~>", b"<|>", b"===", b"!==",
    b"...", b"+++", b"---", b"^^^", b"~~~", b"==", b"!=", b"<=", b">=", b"=~", b"&&", b"||", b"|>",
    b"->", b"<-", b"\\\\", b"::", b"..", b"++", b"--", b"<>", b"=>", b"**", b"<~", b"~>", b"<<",
    b">>", b"!", b"^", b"&", b"=", b"<", b">", b"+", b"-", b"*", b"/", b"|", b".", b"@",
];

fn operator_len(bytes: &[u8]) -> usize {
    OPERATORS
        .iter()
        .find(|op| bytes.starts_with(op))
        .map_or(0, |op| op.len())
}

fn number_end(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0' && matches!(bytes.get(i + 1), Some(b'x' | b'o' | b'b')) {
        let base = bytes[i + 1];
        i += 2;
        while bytes.get(i).is_some_and(|b| {
            *b == b'_'
                || match base {
                    b'x' => b.is_ascii_hexdigit(),
                    b'o' => (b'0'..=b'7').contains(b),
                    _ => matches!(b, b'0' | b'1'),
                }
        }) {
            i += 1;
        }
        return i;
    }
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
    {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
        {
            i += 1;
        }
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if bytes.get(j).is_some_and(u8::is_ascii_digit) {
            i = j + 1;
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
            {
                i += 1;
            }
        }
    }
    i
}

fn literal(kind: ElixirKind, close: u8) -> Literal {
    Literal {
        kind,
        open: 0,
        close,
        depth: 1,
        fence: false,
        interpolate: true,
        braces: 0,
        inner_quote: 0,
    }
}

fn sigil(bytes: &[u8], start: usize) -> Option<(usize, Literal)> {
    let first = *bytes.get(start + 1)?;
    let mut i = start + 2;
    if first.is_ascii_uppercase() {
        while bytes.get(i).is_some_and(u8::is_ascii_uppercase) {
            i += 1;
        }
    } else if !first.is_ascii_lowercase() {
        return None;
    }
    let open = *bytes.get(i)?;
    let close = match open {
        b'/' | b'|' | b'"' | b'\'' => open,
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        _ => return None,
    };
    let mut host = literal(ElixirKind::Sigil, close);
    host.interpolate = first.is_ascii_lowercase();
    host.open = if open != close { open } else { 0 };
    host.fence = matches!(open, b'"' | b'\'') && bytes.get(i..i + 3) == Some(&[open; 3]);
    i += if host.fence { 3 } else { 1 };
    Some((i, host))
}

fn resume_literal(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ElixirState,
    mut host: Literal,
) -> ElixirKind {
    // A heredoc's closing quotes are the first non-blank text of their line.
    let mut blank = *i == 0 || newline(bytes[*i - 1]);
    while *i < bytes.len() && !newline(bytes[*i]) {
        let b = bytes[*i];
        let line_start = blank;
        blank &= matches!(b, b' ' | b'\t');
        if b == b'\\' {
            *i += 1;
            if *i < bytes.len() && !newline(bytes[*i]) {
                *i += 1;
                while bytes.get(*i).is_some_and(|b| b & 0xc0 == 0x80) {
                    *i += 1;
                }
            }
            continue;
        }
        if host.braces != 0 {
            if host.inner_quote != 0 {
                if b == host.inner_quote {
                    host.inner_quote = 0;
                }
            } else {
                match b {
                    b'"' | b'\'' => host.inner_quote = b,
                    b'{' => host.braces += 1,
                    b'}' => host.braces -= 1,
                    _ => {}
                }
            }
            *i += 1;
            continue;
        }
        if host.interpolate && b == b'#' && bytes.get(*i + 1) == Some(&b'{') {
            host.braces = 1;
            *i += 2;
            continue;
        }
        let closes = b == host.close
            && (!host.fence || (line_start && bytes.get(*i..*i + 3) == Some(&[b; 3])));
        if closes {
            host.depth -= 1;
            if host.depth == 0 {
                *i += if host.fence { 3 } else { 1 };
                state.mode = Mode::Normal;
                if host.kind == ElixirKind::Sigil {
                    while bytes.get(*i).is_some_and(u8::is_ascii_alphabetic) {
                        *i += 1;
                    }
                } else if host.kind == ElixirKind::String && key_suffix(bytes, *i) {
                    *i += 1;
                    return ElixirKind::KeywordKey;
                }
                return host.kind;
            }
        } else if host.open != 0 && b == host.open {
            host.depth += 1;
        }
        *i += 1;
    }
    state.mode = Mode::Literal(host);
    host.kind
}

fn lex_one(bytes: &[u8], i: &mut usize, state: &mut ElixirState) -> ElixirToken {
    let start = *i;
    let b = bytes[*i];
    let kind = if newline(b) {
        *i += 1;
        if b == b'\r' && bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        ElixirKind::Newline
    } else if let Mode::Literal(host) = state.mode {
        resume_literal(bytes, i, state, host)
    } else if b.is_ascii_whitespace() {
        *i += 1;
        while bytes
            .get(*i)
            .is_some_and(|b| b.is_ascii_whitespace() && !newline(*b))
        {
            *i += 1;
        }
        ElixirKind::Whitespace
    } else if b == b'#' {
        while *i < bytes.len() && !newline(bytes[*i]) {
            *i += 1;
        }
        ElixirKind::Comment
    } else if b == b'?' && bytes.get(*i + 1).is_some_and(|b| !b.is_ascii_whitespace()) {
        *i += 1;
        if bytes[*i] == b'\\' {
            *i += 1;
        }
        if *i < bytes.len() && !newline(bytes[*i]) {
            *i += 1;
            while bytes.get(*i).is_some_and(|b| b & 0xc0 == 0x80) {
                *i += 1;
            }
        }
        ElixirKind::Char
    } else if b == b'~' && sigil(bytes, *i).is_some() {
        let (end, host) = sigil(bytes, *i).unwrap();
        *i = end;
        if host.fence {
            while *i < bytes.len() && !newline(bytes[*i]) {
                *i += 1;
            }
            state.mode = Mode::Literal(host);
            host.kind
        } else {
            resume_literal(bytes, i, state, host)
        }
    } else if matches!(b, b'"' | b'\'') {
        let fence = bytes.get(*i..*i + 3) == Some(&[b; 3]);
        let kind = if fence {
            ElixirKind::Heredoc
        } else if b == b'"' {
            ElixirKind::String
        } else {
            ElixirKind::Charlist
        };
        let mut host = literal(kind, b);
        host.fence = fence;
        *i += if fence { 3 } else { 1 };
        if fence {
            while *i < bytes.len() && !newline(bytes[*i]) {
                *i += 1;
            }
            state.mode = Mode::Literal(host);
            kind
        } else {
            resume_literal(bytes, i, state, host)
        }
    } else if b == b':' && bytes.get(*i + 1) != Some(&b':') {
        match bytes.get(*i + 1).copied() {
            Some(q @ (b'"' | b'\'')) => {
                *i += 2;
                resume_literal(bytes, i, state, literal(ElixirKind::Atom, q))
            }
            Some(c) if ident_start(c) => {
                *i = ident_end(bytes, *i + 1, true);
                ElixirKind::Atom
            }
            _ => {
                let n = operator_len(&bytes[*i + 1..]);
                *i += 1 + n;
                if n == 0 {
                    ElixirKind::Unknown
                } else {
                    ElixirKind::Atom
                }
            }
        }
    } else if b == b'@' && bytes.get(*i + 1).is_some_and(|b| ident_start(*b)) {
        *i = ident_end(bytes, *i + 1, true);
        ElixirKind::Attribute
    } else if ident_start(b) {
        *i = ident_end(bytes, *i, !b.is_ascii_uppercase());
        if key_suffix(bytes, *i) {
            *i += 1;
            ElixirKind::KeywordKey
        } else if is_keyword(&bytes[start..*i]) {
            ElixirKind::Keyword
        } else if b.is_ascii_uppercase() {
            ElixirKind::Alias
        } else {
            ElixirKind::Identifier
        }
    } else if b.is_ascii_digit() {
        *i = number_end(bytes, *i);
        ElixirKind::Number
    } else if matches!(
        b,
        b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b';' | b'%'
    ) {
        *i += 1;
        if b == b'%' && bytes.get(*i) == Some(&b'{') {
            *i += 1;
        }
        ElixirKind::Delimiter
    } else {
        let n = operator_len(&bytes[*i..]);
        *i += n.max(1);
        if n == 0 {
            while bytes.get(*i).is_some_and(|b| b & 0xc0 == 0x80) {
                *i += 1;
            }
            ElixirKind::Unknown
        } else if matches!(&bytes[start..*i], b"<<" | b">>") {
            ElixirKind::Delimiter
        } else {
            ElixirKind::Operator
        }
    };
    ElixirToken {
        start: start as u32,
        end: *i as u32,
        kind,
    }
}

pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<ElixirToken>, LexError> {
    lex_document_from(bytes, ElixirState::default(), cancel)
}

/// End of the literal starting at `start`, including all continuation lines.
/// Structural consumers can join per-line tokens without duplicating the
/// delimiter/interpolation state machine or allocating another token stream.
pub fn literal_end(bytes: &[u8], start: u32) -> u32 {
    let mut i = start as usize;
    let mut state = ElixirState::default();
    while i < bytes.len() {
        lex_one(bytes, &mut i, &mut state);
        if state.mode == Mode::Normal {
            break;
        }
    }
    i as u32
}

pub fn lex_document_from(
    bytes: &[u8],
    mut state: ElixirState,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<ElixirToken>, LexError> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        let token = lex_one(bytes, &mut i, &mut state);
        if token.end <= token.start {
            return Err(LexError::Nonprogress { at: token.start });
        }
        tokens.push(token);
    }
    Ok(tokens)
}

pub fn lex_line(line: &str, incoming: ElixirState) -> (ElixirState, Vec<(usize, TokenRole)>) {
    let mut state = incoming;
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let token = lex_one(line.as_bytes(), &mut i, &mut state);
        if token.end <= token.start {
            break;
        }
        tokens.push(token);
    }
    (state, tokens_to_roles(line.as_bytes(), &tokens))
}

fn token_role(bytes: &[u8], tokens: &[ElixirToken], i: usize) -> TokenRole {
    let t = tokens[i];
    if t.kind == ElixirKind::Keyword {
        match &bytes[t.start as usize..t.end as usize] {
            b"if" | b"unless" | b"case" | b"cond" | b"with" | b"else" => TokenRole::BranchKeyword,
            b"for" | b"receive" => TokenRole::LoopKeyword,
            b"true" | b"false" | b"nil" => TokenRole::Constant,
            _ => TokenRole::Keyword,
        }
    } else if t.kind == ElixirKind::Identifier
        && tokens.get(i + 1).is_some_and(|next| {
            next.start == t.end
                && next.kind == ElixirKind::Delimiter
                && bytes.get(next.start as usize) == Some(&b'(')
        })
    {
        TokenRole::Function
    } else {
        t.kind.role()
    }
}

pub fn document_spans(bytes: &[u8], tokens: &[ElixirToken]) -> Vec<TokenSpan> {
    tokens
        .iter()
        .enumerate()
        .map(|(i, t)| TokenSpan::new(t.start, t.end, token_role(bytes, tokens, i)))
        .collect()
}

fn tokens_to_roles(bytes: &[u8], tokens: &[ElixirToken]) -> Vec<(usize, TokenRole)> {
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        push_role(t.end as usize, token_role(bytes, tokens, i), &mut out);
    }
    out
}

fn push_role(end: usize, role: TokenRole, out: &mut Vec<(usize, TokenRole)>) {
    if let Some(last) = out.last_mut().filter(|(_, r)| *r == role) {
        last.0 = end;
    } else {
        out.push((end, role));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_continuations() {
        for (open, middle, close) in [
            ("\"\"\"", "body ~S\"\"\" quoted", "  \"\"\""),
            ("~w[one", "[two]", "]a"),
            ("\"one", "two", "three\""),
        ] {
            let (state, roles) = lex_line(open, ElixirState::default());
            assert_ne!(
                state,
                ElixirState::default(),
                "opener carries state: {open}"
            );
            assert_eq!(
                roles.last(),
                Some(&(open.len(), TokenRole::String)),
                "opener is string coloured"
            );
            let (state, roles) = lex_line(middle, state);
            assert_ne!(state, ElixirState::default(), "middle continues: {middle}");
            assert_eq!(
                roles,
                vec![(middle.len(), TokenRole::String)],
                "middle is a single literal"
            );
            let (state, _) = lex_line(close, state);
            assert_eq!(
                state,
                ElixirState::default(),
                "closer resets state: {close}"
            );
        }
    }

    #[test]
    fn atoms_keys_chars_and_roles() {
        let src = b"?# do: as: :ok :: foo( Foo 1..10 ~r/x/iu :<> foo:bar";
        let tokens = lex_document_cancellable(src, &|| false).unwrap();
        let spans = document_spans(src, &tokens);
        for (text, kind, role) in [
            ("?#", ElixirKind::Char, TokenRole::Char),
            ("do:", ElixirKind::KeywordKey, TokenRole::Constant),
            ("as:", ElixirKind::KeywordKey, TokenRole::Constant),
            (":ok", ElixirKind::Atom, TokenRole::Constant),
            ("::", ElixirKind::Operator, TokenRole::Punctuator),
            ("foo", ElixirKind::Identifier, TokenRole::Function),
            ("Foo", ElixirKind::Alias, TokenRole::Typename),
            ("~r/x/iu", ElixirKind::Sigil, TokenRole::String),
        ] {
            let i = tokens
                .iter()
                .position(|t| &src[t.start as usize..t.end as usize] == text.as_bytes())
                .unwrap();
            assert_eq!(tokens[i].kind, kind, "token kind for {text}");
            assert_eq!(spans[i].role, role, "token role for {text}");
        }
        assert!(
            tokens.windows(2).all(|w| w[0].end == w[1].start),
            "tokens cover every byte once"
        );
        assert_eq!(
            tokens.last().unwrap().end as usize,
            src.len(),
            "coverage reaches EOF"
        );
    }

    #[test]
    fn interpolation_keeps_nested_quotes_and_braces() {
        let src = br##""#{%{a: "}"}}" <> "done" # comment"##;
        let tokens = lex_document_cancellable(src, &|| false).unwrap();
        assert_eq!(
            &src[..tokens[0].end as usize],
            br##""#{%{a: "}"}}""##,
            "inner quote cannot terminate the host"
        );
        assert_eq!(
            tokens[0].kind,
            ElixirKind::String,
            "interpolation remains opaque"
        );
        assert_eq!(
            tokens.last().unwrap().kind,
            ElixirKind::Comment,
            "normal lexing resumes after host"
        );
        let src = b"\"a\nb\"\r\n~w[x\ny]a";
        let tokens = lex_document_cancellable(src, &|| false).unwrap();
        assert!(
            tokens.iter().all(|t| t.kind == ElixirKind::Newline
                || !src[t.start as usize..t.end as usize]
                    .iter()
                    .any(|b| newline(*b))),
            "only newline tokens contain line breaks"
        );
    }
}
