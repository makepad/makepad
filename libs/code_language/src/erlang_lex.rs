//! Source-only Erlang lexer shared by the editor and structural frontend.
//! One state machine handles document and display-line continuation. Literal
//! tokens stop at each newline; the continuation owns their delimiter state.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

pub const ERLANG_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum Mode {
    #[default]
    Normal,
    Literal {
        open: u8,
        close: u8,
        depth: usize,
        triple: bool,
        raw: bool,
        kind: ErlangKind,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ErlangState {
    mode: Mode,
    first_line: bool,
}

impl ErlangState {
    /// An escript's first physical line may start with a shebang.
    pub fn escript() -> Self {
        Self {
            first_line: true,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErlangKind {
    Whitespace,
    Newline,
    Comment,
    Atom,
    Variable,
    Keyword,
    Macro,
    Record,
    Attribute,
    Number,
    Char,
    String,
    Sigil,
    Operator,
    Delimiter,
    Dot,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErlangToken {
    pub start: u32,
    pub end: u32,
    pub kind: ErlangKind,
}

impl ErlangKind {
    pub fn role(self) -> TokenRole {
        match self {
            Self::Whitespace | Self::Newline => TokenRole::Whitespace,
            Self::Comment => TokenRole::Comment,
            Self::Atom | Self::Variable => TokenRole::Identifier,
            Self::Keyword => TokenRole::Keyword,
            Self::Macro => TokenRole::Macro,
            Self::Record => TokenRole::Typename,
            Self::Attribute => TokenRole::Preprocessor,
            Self::Number => TokenRole::Number,
            Self::Char => TokenRole::Char,
            Self::String | Self::Sigil => TokenRole::String,
            Self::Operator | Self::Dot => TokenRole::Punctuator,
            Self::Delimiter => TokenRole::Delimiter,
            Self::Unknown => TokenRole::Unknown,
        }
    }
}

fn keyword(s: &[u8]) -> bool {
    matches!(
        s,
        b"after"
            | b"and"
            | b"andalso"
            | b"band"
            | b"begin"
            | b"bnot"
            | b"bor"
            | b"bsl"
            | b"bsr"
            | b"bxor"
            | b"case"
            | b"catch"
            | b"cond"
            | b"div"
            | b"else"
            | b"end"
            | b"fun"
            | b"if"
            | b"let"
            | b"maybe"
            | b"not"
            | b"of"
            | b"or"
            | b"orelse"
            | b"receive"
            | b"rem"
            | b"try"
            | b"when"
            | b"xor"
    )
}

fn newline(b: u8) -> bool {
    matches!(b, b'\r' | b'\n')
}

fn space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | 0x0b | 0x0c)
}

fn identifier(bytes: &[u8], mut i: usize, atom: bool) -> usize {
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || (atom && *b == b'@'))
    {
        i += 1;
    }
    i
}

fn scalar_end(bytes: &[u8], i: usize) -> usize {
    let mut end = (i + 1).min(bytes.len());
    while bytes.get(end).is_some_and(|b| b & 0xc0 == 0x80) {
        end += 1;
    }
    end
}

/// Consume an escape without ever consuming a physical newline.
fn escape_end(bytes: &[u8], mut i: usize) -> usize {
    i += 1; // backslash
    let Some(&b) = bytes.get(i).filter(|b| !newline(**b)) else {
        return i;
    };
    i += 1;
    match b {
        b'x' if bytes.get(i) == Some(&b'{') => {
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_hexdigit) {
                i += 1;
            }
            if bytes.get(i) == Some(&b'}') {
                i += 1;
            }
        }
        b'x' => {
            for _ in 0..2 {
                if bytes.get(i).is_some_and(u8::is_ascii_hexdigit) {
                    i += 1;
                }
            }
        }
        b'0'..=b'7' => {
            for _ in 0..2 {
                if bytes.get(i).is_some_and(|b| matches!(b, b'0'..=b'7')) {
                    i += 1;
                }
            }
        }
        b'^' => {
            if bytes.get(i).is_some_and(|b| !newline(*b)) {
                i = scalar_end(bytes, i);
            }
        }
        _ => {
            i = scalar_end(bytes, i - 1);
        }
    }
    i
}

fn number_end(bytes: &[u8], mut i: usize) -> usize {
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
    {
        i += 1;
    }
    if bytes.get(i) == Some(&b'#') && bytes.get(i + 1).is_some_and(u8::is_ascii_alphanumeric) {
        return identifier(bytes, i + 1, false);
    }
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
        {
            i += 1;
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
    }
    i
}

fn literal(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ErlangState,
    line_start: &mut bool,
) -> ErlangKind {
    let Mode::Literal {
        open,
        close,
        mut depth,
        triple,
        raw,
        kind,
    } = state.mode
    else {
        return ErlangKind::Unknown;
    };
    while *i < bytes.len() && !newline(bytes[*i]) {
        let b = bytes[*i];
        if triple {
            // A triple quote closes at the first non-space position of a line.
            if *line_start && bytes[*i..].starts_with(b"\"\"\"") {
                *i += 3;
                *line_start = false;
                state.mode = Mode::Normal;
                return kind;
            }
        } else if b == close {
            if depth == 0 {
                *i += 1;
                *line_start = false;
                state.mode = Mode::Normal;
                return kind;
            }
            depth -= 1;
        } else if open != close && b == open {
            depth += 1;
        }
        if !space(b) {
            *line_start = false;
        }
        if b == b'\\' && !raw {
            *i = escape_end(bytes, *i);
        } else {
            *i += 1;
        }
    }
    state.mode = Mode::Literal {
        open,
        close,
        depth,
        triple,
        raw,
        kind,
    };
    kind
}

fn start_literal(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ErlangState,
    kind: ErlangKind,
    raw: bool,
    line_start: &mut bool,
) -> ErlangKind {
    let open = bytes[*i];
    let close = match open {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        _ => open,
    };
    let triple = open == b'"' && bytes[*i..].starts_with(b"\"\"\"");
    *i += if triple { 3 } else { 1 };
    *line_start = false;
    state.mode = Mode::Literal {
        open,
        close,
        depth: 0,
        triple,
        raw,
        kind,
    };
    literal(bytes, i, state, line_start)
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ErlangState,
    line_start: &mut bool,
) -> ErlangKind {
    let b = bytes[*i];
    if newline(b) {
        *i += 1;
        if b == b'\r' && bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        state.first_line = false;
        *line_start = true;
        return ErlangKind::Newline;
    }
    if state.mode != Mode::Normal {
        return literal(bytes, i, state, line_start);
    }
    if b == b'%' || (state.first_line && *i == 0 && bytes.starts_with(b"#!")) {
        while *i < bytes.len() && !newline(bytes[*i]) {
            *i += 1;
        }
        *line_start = false;
        return ErlangKind::Comment;
    }
    if space(b) {
        while bytes.get(*i).is_some_and(|b| space(*b)) {
            *i += 1;
        }
        return ErlangKind::Whitespace;
    }
    let at_line_start = *line_start;
    *line_start = false;
    if b.is_ascii_digit() {
        *i = number_end(bytes, *i);
        return ErlangKind::Number;
    }
    if b == b'$' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\\') {
            *i = escape_end(bytes, *i);
        } else if bytes.get(*i).is_some_and(|b| !newline(*b)) {
            *i = scalar_end(bytes, *i);
        }
        return ErlangKind::Char;
    }
    if b == b'"' || b == b'\'' {
        let kind = if b == b'"' {
            ErlangKind::String
        } else {
            ErlangKind::Atom
        };
        return start_literal(bytes, i, state, kind, false, line_start);
    }
    if b == b'~' {
        let mut j = *i + 1;
        let raw = matches!(bytes.get(j), Some(b'S' | b'B'));
        if matches!(bytes.get(j), Some(b's' | b'b' | b'S' | b'B')) {
            j += 1;
        }
        if matches!(
            bytes.get(j),
            Some(b'(' | b'[' | b'{' | b'<' | b'/' | b'|' | b'\'' | b'"' | b'`' | b'#')
        ) {
            *i = j;
            return start_literal(bytes, i, state, ErlangKind::Sigil, raw, line_start);
        }
    }
    let prefix_kind = match b {
        b'-' if at_line_start => Some(ErlangKind::Attribute),
        b'?' => Some(ErlangKind::Macro),
        b'#' => Some(ErlangKind::Record),
        _ => None,
    };
    if let Some(kind) = prefix_kind {
        let mut j = *i + 1;
        if kind == ErlangKind::Macro && bytes.get(j) == Some(&b'?') {
            j += 1;
        }
        if bytes.get(j) == Some(&b'\'') {
            *i = j;
            return start_literal(bytes, i, state, kind, false, line_start);
        }
        if bytes.get(j).is_some_and(|b| {
            b.is_ascii_lowercase()
                || (kind == ErlangKind::Macro && (b.is_ascii_uppercase() || *b == b'_'))
        }) {
            let atom = bytes[j].is_ascii_lowercase();
            *i = identifier(bytes, j + 1, atom);
            return kind;
        }
    }
    if b.is_ascii_alphabetic() || b == b'_' {
        let start = *i;
        let atom = b.is_ascii_lowercase();
        *i = identifier(bytes, start + 1, atom);
        return if !atom {
            ErlangKind::Variable
        } else if keyword(&bytes[start..*i]) {
            ErlangKind::Keyword
        } else {
            ErlangKind::Atom
        };
    }
    if bytes[*i..].starts_with(b"#{")
        || bytes[*i..].starts_with(b"<<")
        || bytes[*i..].starts_with(b">>")
    {
        *i += 2;
        return ErlangKind::Delimiter;
    }
    if matches!(b, b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b';') {
        *i += 1;
        return ErlangKind::Delimiter;
    }
    if b == b'.'
        && bytes
            .get(*i + 1)
            .is_none_or(|b| b.is_ascii_whitespace() || *b == b'%')
    {
        *i += 1;
        return ErlangKind::Dot;
    }
    for op in [
        "=:=", "=/=", "...", "->", "<-", "<=", "=>", "=<", ">=", "==", "/=", "++", "--", "||",
        "::", ":=", "!!", "..", "?=", "!", "|", "+", "-", "*", "/", "=", "<", ">", ":", "#", ".",
    ] {
        if bytes[*i..].starts_with(op.as_bytes()) {
            *i += op.len();
            return ErlangKind::Operator;
        }
    }
    *i = scalar_end(bytes, *i);
    ErlangKind::Unknown
}

pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<ErlangToken>, LexError> {
    lex_document_from(bytes, ErlangState::default(), cancel)
}

pub fn lex_document_from(
    bytes: &[u8],
    mut state: ErlangState,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<ErlangToken>, LexError> {
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut line_start = true;
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        let start = i;
        let kind = lex_one(bytes, &mut i, &mut state, &mut line_start);
        if i <= start || i > u32::MAX as usize {
            return Err(LexError::Nonprogress { at: start as u32 });
        }
        tokens.push(ErlangToken {
            start: start as u32,
            end: i as u32,
            kind,
        });
    }
    Ok(tokens)
}

/// Each `(end, role)` covers the bytes since the preceding end.
pub fn lex_line(line: &str, mut incoming: ErlangState) -> (ErlangState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut line_start = true;
    while i < bytes.len() {
        let start = i;
        let mut kind = lex_one(bytes, &mut i, &mut incoming, &mut line_start);
        if i <= start {
            i = scalar_end(bytes, start);
            kind = ErlangKind::Unknown;
        }
        tokens.push(ErlangToken {
            start: start as u32,
            end: i as u32,
            kind,
        });
    }
    incoming.first_line = false;
    (incoming, tokens_to_roles(bytes, &tokens))
}

fn token_role(bytes: &[u8], tokens: &[ErlangToken], i: usize) -> TokenRole {
    let t = tokens[i];
    let text = &bytes[t.start as usize..t.end as usize];
    if t.kind == ErlangKind::Keyword
        && matches!(
            text,
            b"case" | b"if" | b"receive" | b"try" | b"maybe" | b"catch" | b"of" | b"after"
        )
    {
        return TokenRole::BranchKeyword;
    }
    if t.kind == ErlangKind::Atom {
        let atom = text
            .strip_prefix(b"'")
            .and_then(|s| s.strip_suffix(b"'"))
            .unwrap_or(text);
        if matches!(atom, b"true" | b"false" | b"undefined" | b"ok" | b"error") {
            return TokenRole::Constant;
        }
        if tokens.get(i + 1).is_some_and(|n| {
            n.start == t.end
                && n.kind == ErlangKind::Delimiter
                && &bytes[n.start as usize..n.end as usize] == b"("
        }) {
            return TokenRole::Function;
        }
    }
    t.kind.role()
}

pub fn document_spans(bytes: &[u8], tokens: &[ErlangToken]) -> Vec<TokenSpan> {
    tokens
        .iter()
        .enumerate()
        .map(|(i, t)| TokenSpan::new(t.start, t.end, token_role(bytes, tokens, i)))
        .collect()
}

fn tokens_to_roles(bytes: &[u8], tokens: &[ErlangToken]) -> Vec<(usize, TokenRole)> {
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        push_role(t.end as usize, token_role(bytes, tokens, i), &mut out);
    }
    out
}

fn push_role(end: usize, role: TokenRole, out: &mut Vec<(usize, TokenRole)>) {
    if let Some(last) = out.last_mut().filter(|last| last.1 == role) {
        last.0 = end;
    } else {
        out.push((end, role));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_continuations_and_roles() {
        for (first, middle, last, kind) in [
            ("\"one", "two", "three\"", ErlangKind::String),
            (
                "\"\"\"",
                "one \"\"\" inside",
                "  \"\"\"",
                ErlangKind::String,
            ),
            ("'one", "two", "three'", ErlangKind::Atom),
            ("~s{one", "{two}", "three}", ErlangKind::Sigil),
        ] {
            let (state, _) = lex_line(first, ErlangState::default());
            assert_ne!(state.mode, Mode::Normal, "{first}: opens a literal");
            let (state, roles) = lex_line(middle, state);
            assert_eq!(
                roles,
                vec![(middle.len(), kind.role())],
                "{first}: continuation role"
            );
            let (state, _) = lex_line(last, state);
            assert_eq!(state.mode, Mode::Normal, "{first}: closes on final line");
            let src = format!("{first}\r\n{middle}\n{last}");
            let ts = lex_document_cancellable(src.as_bytes(), &|| false).unwrap();
            assert_eq!(
                ts.iter().filter(|t| t.kind == ErlangKind::Newline).count(),
                2,
                "newlines are separate tokens"
            );
            assert!(
                ts.windows(2).all(|w| w[0].end == w[1].start),
                "no gaps or overlaps"
            );
            assert_eq!(
                ts.last().unwrap().end as usize,
                src.len(),
                "every byte covered"
            );
        }
        let src = "f() f ()\nf\n() case ok";
        let ts = lex_document_cancellable(src.as_bytes(), &|| false).unwrap();
        let spans = document_spans(src.as_bytes(), &ts);
        assert_eq!(
            spans
                .iter()
                .filter(|t| t.role == TokenRole::Function)
                .count(),
            1,
            "only adjacent parentheses colour a function"
        );
        assert!(
            spans.iter().any(|t| t.role == TokenRole::BranchKeyword),
            "branch keywords"
        );
        assert!(
            spans.iter().any(|t| t.role == TokenRole::Constant),
            "constant atoms"
        );
    }

    #[test]
    fn punctuation_literals_and_prefixes() {
        let src = "  -module(sample).\nf(X) -> $%, $ , $\\n, $\\\\, $\\^A, $\\x{41}, X - 1, ?MODULE, ??Arg, #state{}, #{}, 16#FF, 1.0e-3, R#r.f.\n";
        let ts = lex_document_cancellable(src.as_bytes(), &|| false).unwrap();
        for (text, kind) in [
            ("-module", ErlangKind::Attribute),
            ("-", ErlangKind::Operator),
            ("?MODULE", ErlangKind::Macro),
            ("??Arg", ErlangKind::Macro),
            ("#state", ErlangKind::Record),
            ("#{", ErlangKind::Delimiter),
            ("16#FF", ErlangKind::Number),
            ("1.0e-3", ErlangKind::Number),
        ] {
            assert!(
                ts.iter()
                    .any(|t| &src[t.start as usize..t.end as usize] == text && t.kind == kind),
                "{text}: {kind:?}"
            );
        }
        assert_eq!(
            ts.iter().filter(|t| t.kind == ErlangKind::Char).count(),
            6,
            "all character escape forms stay whole"
        );
        assert!(
            !ts.iter().any(|t| t.kind == ErlangKind::Comment),
            "$% is not a comment"
        );
        assert_eq!(
            ts.iter().filter(|t| t.kind == ErlangKind::Dot).count(),
            2,
            "only form terminators are dots"
        );
        assert!(
            ts.iter()
                .any(|t| t.kind == ErlangKind::Operator
                    && &src[t.start as usize..t.end as usize] == "."),
            "record field dot is an operator"
        );
    }

    #[test]
    fn escript_first_line_only() {
        let src = "#!/usr/bin/env escript\nmain(_) -> ok.\n#!again";
        let ts = lex_document_from(src.as_bytes(), ErlangState::escript(), &|| false).unwrap();
        assert_eq!(ts[0].kind, ErlangKind::Comment, "shebang is a comment");
        assert_eq!(
            ts.iter().filter(|t| t.kind == ErlangKind::Comment).count(),
            1,
            "only first line accepts a shebang"
        );
        let (state, _) = lex_line("", ErlangState::escript());
        let (_, roles) = lex_line("#!later", state);
        assert!(
            !roles.iter().any(|r| r.1 == TokenRole::Comment),
            "an empty first line clears the flag"
        );
    }
}
