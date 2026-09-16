//! Source-only JavaScript / TypeScript lexer shared by the editor and the
//! script frontend. Byte offsets address the original document. Continuation
//! state is provider-owned: template interpolations and JSX nesting are not
//! packed into a universal integer.
//!
//! JSX is on for every JavaScript dialect (`Js`, `Jsx`, `Tsx`) and off for
//! `Ts` / `Dts`. `ScriptState::default` stays Js with JSX enabled. A string
//! that ends at `\` on a line keeps `DoubleString` / `SingleString` so
//! `lex_line` resumes it; an unescaped end of line still resets.

use crate::id::{Dialect, LanguageId};
use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const SCRIPT_LEXER_VERSION: u32 = 1;

/// Nested template / JSX frames kept in line continuation. Capped so a
/// pathological file cannot grow unbounded state.
const FRAME_CAP: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScriptDialect {
    Js,
    Jsx,
    Ts,
    Tsx,
    Dts,
}

impl ScriptDialect {
    pub fn from_dialect(d: Dialect) -> Self {
        match (d.language, d.name) {
            (LanguageId::TypeScript, "tsx") => ScriptDialect::Tsx,
            (LanguageId::TypeScript, "dts") => ScriptDialect::Dts,
            (LanguageId::TypeScript, _) => ScriptDialect::Ts,
            (LanguageId::JavaScript, "jsx") => ScriptDialect::Jsx,
            _ => ScriptDialect::Js,
        }
    }

    pub fn jsx(self) -> bool {
        matches!(
            self,
            ScriptDialect::Js | ScriptDialect::Jsx | ScriptDialect::Tsx
        )
    }

    pub fn typescript(self) -> bool {
        matches!(
            self,
            ScriptDialect::Ts | ScriptDialect::Tsx | ScriptDialect::Dts
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
enum ScriptMode {
    Normal = 0,
    BlockComment = 1,
    DoubleString = 2,
    SingleString = 3,
    Template = 4,
    Interpolation = 5,
    JsxTag = 6,
    JsxChildren = 7,
    JsxExpr = 8,
}

/// Previous significant token class, used for `/` regex vs division and for
/// JSX `<` vs comparison / generic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
enum PrevSig {
    None = 0,
    /// `)` `]` `}`
    Close = 1,
    /// Any other punctuator or opener `( [ {`
    Punct = 2,
    Ident = 3,
    Number = 4,
    /// String, template chunk, regex
    String = 5,
    /// `return typeof instanceof in of new delete void throw case do else yield await`
    RegexKw = 6,
}

/// Provider-owned line continuation. Default dialect is Js with JSX enabled.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScriptState {
    dialect: ScriptDialect,
    /// JSX enabled independently of `dialect` so Default can be Js+JSX while
    /// `for_dialect(Ts)` keeps `<T>expr` as an assertion, never an element.
    jsx: bool,
    mode: ScriptMode,
    /// Mode to restore when a string or block-comment overlay ends.
    resume: ScriptMode,
    /// Brace depths of open template interpolations and JSX `{` expressions,
    /// innermost last. Kind is the matching `frames` / `mode` entry.
    braces: Vec<u16>,
    /// Suspended outer modes (template / interpolation / JSX), innermost last.
    frames: Vec<u8>,
    jsx_depth: u16,
    prev: u8,
    /// True until the first token of a document / first line; hashbang only then.
    at_start: bool,
    /// True while lexing `</name>` so `>` closes the element rather than
    /// entering children.
    jsx_closing: bool,
    /// True when the last significant token inside a JSX tag was `/`.
    /// Survives the line boundary so `<div\n/>` is still self-closing.
    jsx_slash: bool,
}

impl Default for ScriptState {
    fn default() -> Self {
        let mut s = ScriptState::for_dialect(ScriptDialect::Js);
        s.jsx = true;
        s
    }
}

impl ScriptState {
    pub fn for_dialect(d: ScriptDialect) -> Self {
        ScriptState {
            dialect: d,
            jsx: d.jsx(),
            mode: ScriptMode::Normal,
            resume: ScriptMode::Normal,
            braces: Vec::new(),
            frames: Vec::new(),
            jsx_depth: 0,
            prev: PrevSig::None as u8,
            at_start: true,
            jsx_closing: false,
            jsx_slash: false,
        }
    }

    fn typescript(&self) -> bool {
        self.dialect.typescript()
    }

    fn push_frame(&mut self, outer: ScriptMode) -> bool {
        if self.frames.len() >= FRAME_CAP {
            return false;
        }
        self.frames.push(outer as u8);
        true
    }

    fn pop_frame(&mut self) -> ScriptMode {
        match self.frames.pop() {
            Some(m) => mode_from_u8(m),
            None => ScriptMode::Normal,
        }
    }

    fn push_braces(&mut self, depth: u16) -> bool {
        if self.braces.len() >= FRAME_CAP {
            return false;
        }
        self.braces.push(depth);
        true
    }
}

fn mode_from_u8(m: u8) -> ScriptMode {
    match m {
        1 => ScriptMode::BlockComment,
        2 => ScriptMode::DoubleString,
        3 => ScriptMode::SingleString,
        4 => ScriptMode::Template,
        5 => ScriptMode::Interpolation,
        6 => ScriptMode::JsxTag,
        7 => ScriptMode::JsxChildren,
        8 => ScriptMode::JsxExpr,
        _ => ScriptMode::Normal,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptKind {
    Whitespace,
    Comment,
    Identifier,
    PrivateName,
    Keyword,
    Number,
    String,
    Template,
    Regex,
    Punctuator,
    Delimiter,
    JsxText,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptToken {
    pub start: u32,
    pub end: u32,
    pub kind: ScriptKind,
}

impl ScriptKind {
    pub fn role(self) -> TokenRole {
        match self {
            ScriptKind::Whitespace => TokenRole::Whitespace,
            ScriptKind::Comment => TokenRole::Comment,
            ScriptKind::Identifier | ScriptKind::PrivateName => TokenRole::Identifier,
            ScriptKind::Keyword => TokenRole::Keyword,
            ScriptKind::Number => TokenRole::Number,
            ScriptKind::String | ScriptKind::Template | ScriptKind::Regex => TokenRole::String,
            ScriptKind::Punctuator => TokenRole::Punctuator,
            ScriptKind::Delimiter => TokenRole::Delimiter,
            ScriptKind::JsxText | ScriptKind::Unknown => TokenRole::Unknown,
        }
    }
}

const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "get",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "set",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

const TS_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "declare",
    "enum",
    "implements",
    "infer",
    "interface",
    "is",
    "keyof",
    "module",
    "namespace",
    "override",
    "private",
    "protected",
    "public",
    "readonly",
    "satisfies",
    "type",
];

pub fn is_keyword(ident: &str, typescript: bool) -> bool {
    JS_KEYWORDS.binary_search(&ident).is_ok()
        || (typescript && TS_KEYWORDS.binary_search(&ident).is_ok())
}

fn keyword_role(ident: &str) -> TokenRole {
    match ident {
        "if" | "else" | "switch" | "case" | "default" | "try" | "catch" | "finally" | "return"
        | "throw" => TokenRole::BranchKeyword,
        "for" | "while" | "do" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "null" => TokenRole::Constant,
        other if JS_KEYWORDS.binary_search(&other).is_ok() || TS_KEYWORDS.binary_search(&other).is_ok() => {
            TokenRole::Keyword
        }
        _ => TokenRole::Identifier,
    }
}

fn classify_identifier(ident: &str, typescript: bool, next_is_paren: bool) -> TokenRole {
    if is_keyword(ident, typescript) {
        return keyword_role(ident);
    }
    match ident {
        "undefined" | "NaN" | "Infinity" => return TokenRole::Constant,
        _ => {}
    }
    if next_is_paren {
        return TokenRole::Function;
    }
    if ident
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        TokenRole::Typename
    } else {
        TokenRole::Identifier
    }
}

fn is_regex_kw(ident: &str) -> bool {
    matches!(
        ident,
        "return"
            | "typeof"
            | "instanceof"
            | "in"
            | "of"
            | "new"
            | "delete"
            | "void"
            | "throw"
            | "case"
            | "do"
            | "else"
            | "yield"
            | "await"
    )
}

fn is_value_kw(ident: &str) -> bool {
    matches!(ident, "true" | "false" | "null" | "this" | "super")
}

fn prev_for_keyword(ident: &str) -> PrevSig {
    if is_regex_kw(ident) {
        PrevSig::RegexKw
    } else if is_value_kw(ident) {
        PrevSig::Ident
    } else {
        PrevSig::Punct
    }
}

fn is_expr_start(prev: PrevSig) -> bool {
    matches!(prev, PrevSig::None | PrevSig::Punct | PrevSig::RegexKw)
}

pub fn lex_document_cancellable(
    bytes: &[u8],
    dialect: ScriptDialect,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<ScriptToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = ScriptState::for_dialect(dialect);
    let mut i = 0usize;
    let token_cap = bytes.len().saturating_add(8);
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if tokens.len() > token_cap {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
        let start_i = i;
        lex_one(bytes, &mut i, &mut state, &mut tokens, cancel)?;
        if i == start_i && i < bytes.len() {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
    }
    Ok(tokens)
}

pub fn lex_line(line: &str, incoming: ScriptState) -> (ScriptState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let cancel = || false;
    while i < bytes.len() {
        let start_i = i;
        if lex_one(bytes, &mut i, &mut state, &mut tokens, &cancel).is_err() {
            if i == start_i {
                i += 1;
                let _ = push_token(&mut tokens, start_i as u32, i, ScriptKind::Unknown);
            }
        }
        if i == start_i && i < bytes.len() {
            i += 1;
            let _ = push_token(&mut tokens, start_i as u32, i, ScriptKind::Unknown);
        }
    }
    let roles = tokens_to_roles(bytes, &tokens, state.typescript());
    (state, roles)
}

pub fn document_spans(bytes: &[u8], tokens: &[ScriptToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    // Keyword vs Identifier is already dialect-aware on the token. This pass
    // only applies role refinements (Function / Typename / Constant / branch).
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if matches!(
            t.kind,
            ScriptKind::Keyword | ScriptKind::Identifier | ScriptKind::PrivateName
        ) {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            let next_is_paren = j < tokens.len()
                && tokens[j].kind == ScriptKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(');
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = match t.kind {
                ScriptKind::PrivateName => TokenRole::Identifier,
                ScriptKind::Keyword => keyword_role(ident),
                _ => classify_identifier(ident, false, next_is_paren),
            };
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}

fn tokens_to_roles(
    bytes: &[u8],
    tokens: &[ScriptToken],
    typescript: bool,
) -> Vec<(usize, TokenRole)> {
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if matches!(
            t.kind,
            ScriptKind::Keyword | ScriptKind::Identifier | ScriptKind::PrivateName
        ) {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            let next_is_paren = j < tokens.len()
                && tokens[j].kind == ScriptKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(');
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = if t.kind == ScriptKind::PrivateName {
                TokenRole::Identifier
            } else if t.kind == ScriptKind::Keyword {
                keyword_role(ident)
            } else {
                classify_identifier(ident, typescript, next_is_paren)
            };
        }
        push_role(t.end as usize, role, &mut out);
    }
    out
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    if *i >= bytes.len() {
        return Ok(());
    }
    let start = *i as u32;
    match state.mode {
        ScriptMode::BlockComment => {
            resume_block_comment(bytes, i, state, tokens, start, cancel)?;
            return Ok(());
        }
        ScriptMode::DoubleString => {
            resume_quoted(bytes, i, state, tokens, start, b'"')?;
            return Ok(());
        }
        ScriptMode::SingleString => {
            resume_quoted(bytes, i, state, tokens, start, b'\'')?;
            return Ok(());
        }
        ScriptMode::Template => {
            resume_template(bytes, i, state, tokens, start)?;
            return Ok(());
        }
        ScriptMode::JsxChildren => {
            resume_jsx_children(bytes, i, state, tokens, start, cancel)?;
            return Ok(());
        }
        ScriptMode::Normal
        | ScriptMode::Interpolation
        | ScriptMode::JsxTag
        | ScriptMode::JsxExpr => {}
    }

    if state.mode == ScriptMode::JsxTag {
        return lex_jsx_tag(bytes, i, state, tokens, start, cancel);
    }

    let b = bytes[*i];

    if state.at_start && b == b'#' && bytes.get(*i + 1) == Some(&b'!') {
        *i += 2;
        while *i < bytes.len() && bytes[*i] != b'\n' && bytes[*i] != b'\r' {
            *i += 1;
        }
        state.at_start = false;
        push_token(tokens, start, *i, ScriptKind::Comment)?;
        set_prev(state, PrevSig::None);
        return Ok(());
    }

    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        state.at_start = false;
        push_token(tokens, start, *i, ScriptKind::Whitespace)?;
        return Ok(());
    }
    if b == b'\n' {
        *i += 1;
        state.at_start = false;
        push_token(tokens, start, *i, ScriptKind::Whitespace)?;
        return Ok(());
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        state.at_start = false;
        push_token(tokens, start, *i, ScriptKind::Whitespace)?;
        return Ok(());
    }

    state.at_start = false;

    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && bytes[*i] != b'\n' && bytes[*i] != b'\r' {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            *i += 1;
        }
        push_token(tokens, start, *i, ScriptKind::Comment)?;
        return Ok(());
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.resume = state.mode;
        state.mode = ScriptMode::BlockComment;
        resume_block_comment(bytes, i, state, tokens, start, cancel)?;
        return Ok(());
    }

    if b == b'/' && !matches!(state.mode, ScriptMode::JsxTag | ScriptMode::JsxChildren) {
        if is_expr_start(prev_of(state)) {
            return lex_regex_or_unknown(bytes, i, state, tokens, start);
        }
        return lex_slash_punct(bytes, i, state, tokens, start);
    }

    if b == b'"' || b == b'\'' {
        let quote = b;
        *i += 1;
        state.resume = state.mode;
        state.mode = if quote == b'"' {
            ScriptMode::DoubleString
        } else {
            ScriptMode::SingleString
        };
        resume_quoted(bytes, i, state, tokens, start, quote)?;
        return Ok(());
    }

    if b == b'`' {
        return start_template(bytes, i, state, tokens, start);
    }

    if b == b'<'
        && state.jsx
        && is_expr_start(prev_of(state))
        && !matches!(state.mode, ScriptMode::JsxTag)
    {
        match jsx_lt_decision(bytes, *i, state) {
            LtDecision::Jsx => return start_jsx(bytes, i, state, tokens, start),
            LtDecision::Generic => {
                *i += 1;
                push_token(tokens, start, *i, ScriptKind::Punctuator)?;
                set_prev(state, PrevSig::Punct);
                return Ok(());
            }
            LtDecision::Unknown => {
                *i += 1;
                push_token(tokens, start, *i, ScriptKind::Unknown)?;
                set_prev(state, PrevSig::Punct);
                return Ok(());
            }
            LtDecision::Punct => {}
        }
    }

    if b == b'#' && *i + 1 < bytes.len() && is_ident_start(bytes[*i + 1])
        || (b == b'#' && bytes.get(*i + 1) == Some(&b'\\'))
    {
        *i += 1;
        *i = scan_ident(bytes, *i);
        push_token(tokens, start, *i, ScriptKind::PrivateName)?;
        set_prev(state, PrevSig::Ident);
        return Ok(());
    }

    if is_ident_start(b) || (b == b'\\' && bytes.get(*i + 1) == Some(&b'u')) {
        *i = scan_ident(bytes, *i);
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident, state.typescript()) {
            ScriptKind::Keyword
        } else {
            ScriptKind::Identifier
        };
        push_token(tokens, start, *i, kind)?;
        if kind == ScriptKind::Keyword {
            set_prev(state, prev_for_keyword(ident));
        } else {
            set_prev(state, PrevSig::Ident);
        }
        return Ok(());
    }

    if b.is_ascii_digit()
        || (b == b'.' && bytes.get(*i + 1).copied().unwrap_or(0).is_ascii_digit())
    {
        *i = scan_number(bytes, *i);
        push_token(tokens, start, *i, ScriptKind::Number)?;
        set_prev(state, PrevSig::Number);
        return Ok(());
    }

    if b == b'{' {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Delimiter)?;
        bump_open_brace(state);
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    if b == b'}' {
        if close_expr_brace(state) {
            *i += 1;
            let kind = if state.mode == ScriptMode::Template {
                ScriptKind::Template
            } else {
                ScriptKind::Delimiter
            };
            push_token(tokens, start, *i, kind)?;
            if kind == ScriptKind::Template {
                set_prev(state, PrevSig::String);
            } else {
                set_prev(state, PrevSig::Close);
            }
            return Ok(());
        }
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Delimiter)?;
        set_prev(state, PrevSig::Close);
        return Ok(());
    }
    if matches!(b, b'(' | b')' | b'[' | b']') {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Delimiter)?;
        if matches!(b, b')' | b']') {
            set_prev(state, PrevSig::Close);
        } else {
            set_prev(state, PrevSig::Punct);
        }
        return Ok(());
    }

    *i += scan_punct_len(bytes, *i);
    if *i as u32 <= start {
        *i = (start as usize) + 1;
    }
    let kind = if bytes[start as usize].is_ascii_graphic() {
        ScriptKind::Punctuator
    } else {
        ScriptKind::Unknown
    };
    push_token(tokens, start, *i, kind)?;
    set_prev(state, PrevSig::Punct);
    Ok(())
}

fn resume_block_comment(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    while *i < bytes.len() {
        if *i & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
            *i += 2;
            state.mode = state.resume;
            break;
        }
        *i += 1;
    }
    if (*i as u32) > start {
        push_token(tokens, start, *i, ScriptKind::Comment)?;
    } else if *i >= bytes.len() {
        // empty remainder; keep BlockComment
    }
    Ok(())
}

fn resume_quoted(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
    quote: u8,
) -> Result<(), LexError> {
    let (closed, end, continued) = finish_string(bytes, *i, quote);
    if end < *i {
        return Err(LexError::Nonprogress { at: start });
    }
    *i = end;
    if end as u32 > start {
        push_token(tokens, start, end, ScriptKind::String)?;
        set_prev(state, PrevSig::String);
    }
    if closed {
        state.mode = state.resume;
    } else if continued {
        // Line ended right after `\`: keep DoubleString/SingleString so the
        // next lex_line resumes the literal.
    } else {
        // Unescaped end of line or EOF: state resets out of the string.
        state.mode = state.resume;
    }
    Ok(())
}

/// Scan from `i` (after the opening quote). Unescaped CR/LF terminates the
/// literal and leaves the cursor on that newline. `\` + newline is a line
/// continuation and stays inside the literal. Returns
/// `(closed, end, continued)` where `continued` is true when the buffer ended
/// immediately after a `\`.
fn finish_string(bytes: &[u8], mut i: usize, quote: u8) -> (bool, usize, bool) {
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += 1;
            if i >= bytes.len() {
                return (false, i, true);
            }
            if bytes[i] == b'\r' {
                i += 1;
                if bytes.get(i) == Some(&b'\n') {
                    i += 1;
                }
                continue;
            }
            if bytes[i] == b'\n' {
                i += 1;
                continue;
            }
            i += 1;
            continue;
        }
        if b == quote {
            return (true, i + 1, false);
        }
        if b == b'\n' || b == b'\r' {
            return (false, i, false);
        }
        i += 1;
    }
    (false, i, false)
}

fn start_template(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
) -> Result<(), LexError> {
    let outer = state.mode;
    if !state.push_frame(outer) {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Unknown)?;
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    state.mode = ScriptMode::Template;
    *i += 1; // opening backtick is part of the chunk
    resume_template(bytes, i, state, tokens, start)
}

fn resume_template(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
) -> Result<(), LexError> {
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b'\\' {
            *i += 1;
            if *i < bytes.len() {
                *i += 1;
            }
            continue;
        }
        if b == b'`' {
            *i += 1;
            push_token(tokens, start, *i, ScriptKind::Template)?;
            set_prev(state, PrevSig::String);
            state.mode = state.pop_frame();
            return Ok(());
        }
        if b == b'$' && bytes.get(*i + 1) == Some(&b'{') {
            *i += 2;
            push_token(tokens, start, *i, ScriptKind::Template)?;
            set_prev(state, PrevSig::Punct);
            if !state.push_frame(ScriptMode::Template) || !state.push_braces(0) {
                state.mode = ScriptMode::Template;
                return Ok(());
            }
            state.mode = ScriptMode::Interpolation;
            return Ok(());
        }
        *i += 1;
    }
    if (*i as u32) > start {
        push_token(tokens, start, *i, ScriptKind::Template)?;
        set_prev(state, PrevSig::String);
    }
    // Stay in Template across the line break.
    Ok(())
}

fn bump_open_brace(state: &mut ScriptState) {
    if matches!(
        state.mode,
        ScriptMode::Interpolation | ScriptMode::JsxExpr
    ) {
        if let Some(d) = state.braces.last_mut() {
            *d = d.saturating_add(1);
        }
    }
}

/// Handle `}` that may close a template interpolation or JSX expression.
/// Returns true when the token was consumed as a nest closer (caller emits).
fn close_expr_brace(state: &mut ScriptState) -> bool {
    match state.mode {
        ScriptMode::Interpolation => {
            match state.braces.last_mut() {
                Some(d) if *d > 0 => {
                    *d -= 1;
                    false
                }
                Some(_) => {
                    state.braces.pop();
                    let _ = state.pop_frame(); // Interpolation's outer was Template
                    state.mode = ScriptMode::Template;
                    true
                }
                None => false,
            }
        }
        ScriptMode::JsxExpr => {
            match state.braces.last_mut() {
                Some(d) if *d > 1 => {
                    *d -= 1;
                    false
                }
                Some(_) => {
                    state.braces.pop();
                    state.mode = state.pop_frame();
                    true
                }
                None => false,
            }
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum LtDecision {
    Jsx,
    Generic,
    Unknown,
    Punct,
}

fn jsx_lt_decision(bytes: &[u8], at: usize, state: &ScriptState) -> LtDecision {
    // `at` points at `<`.
    let mut k = at + 1;
    k = skip_ws(bytes, k);
    if k >= bytes.len() {
        return LtDecision::Unknown;
    }
    let n = bytes[k];
    if n == b'>' || n == b'{' {
        return LtDecision::Jsx;
    }
    if n == b'/' {
        // `</` only valid inside JSX children, handled there.
        return LtDecision::Punct;
    }
    if !(is_ident_start(n) || n == b'\\') {
        return LtDecision::Punct;
    }
    k = scan_jsx_tag_name(bytes, k);
    k = skip_ws(bytes, k);
    if k >= bytes.len() {
        return LtDecision::Unknown;
    }
    // TSX generics in a non-JSX context: identifier then `,` / `extends` /
    // `>` followed by `(`.
    if state.typescript() && state.jsx && !matches!(state.mode, ScriptMode::JsxChildren) {
        if bytes[k] == b',' {
            return LtDecision::Generic;
        }
        if is_ident_start(bytes[k]) {
            let ident_end = scan_ident(bytes, k);
            let ident = std::str::from_utf8(&bytes[k..ident_end]).unwrap_or("");
            if ident == "extends" {
                return LtDecision::Generic;
            }
        }
        if bytes[k] == b'>' {
            let n = skip_ws(bytes, k + 1);
            if bytes.get(n) == Some(&b'(') {
                return LtDecision::Generic;
            }
        }
    }
    LtDecision::Jsx
}

fn start_jsx(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
) -> Result<(), LexError> {
    let outer = state.mode;
    if !state.push_frame(outer) {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Unknown)?;
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    state.mode = ScriptMode::JsxTag;
    state.jsx_closing = false;
    state.jsx_slash = false;
    state.jsx_depth = state.jsx_depth.saturating_add(1);
    *i += 1;
    push_token(tokens, start, *i, ScriptKind::Punctuator)?;
    set_prev(state, PrevSig::Punct);
    let _ = bytes;
    Ok(())
}

fn lex_jsx_tag(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    let b = bytes[*i];
    if is_space(b) || b == b'\n' || b == b'\r' {
        *i += 1;
        while *i < bytes.len() && (is_space(bytes[*i]) || bytes[*i] == b'\n' || bytes[*i] == b'\r') {
            *i += 1;
        }
        push_token(tokens, start, *i, ScriptKind::Whitespace)?;
        return Ok(());
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && bytes[*i] != b'\n' && bytes[*i] != b'\r' {
            *i += 1;
        }
        push_token(tokens, start, *i, ScriptKind::Comment)?;
        return Ok(());
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.resume = state.mode;
        state.mode = ScriptMode::BlockComment;
        resume_block_comment(bytes, i, state, tokens, start, cancel)?;
        return Ok(());
    }
    if b == b'"' || b == b'\'' {
        let quote = b;
        *i += 1;
        state.jsx_slash = false;
        state.resume = state.mode;
        state.mode = if quote == b'"' {
            ScriptMode::DoubleString
        } else {
            ScriptMode::SingleString
        };
        resume_quoted(bytes, i, state, tokens, start, quote)?;
        return Ok(());
    }
    if b == b'{' {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Delimiter)?;
        state.jsx_slash = false;
        if state.push_frame(ScriptMode::JsxTag) && state.push_braces(1) {
            state.mode = ScriptMode::JsxExpr;
        }
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    if b == b'/' {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Punctuator)?;
        state.jsx_slash = true;
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    if b == b'>' {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Punctuator)?;
        // Self-closing is `/` then `>`: `jsx_slash` is set when `/` is lexed
        // inside the tag and survives the line boundary (the per-line token
        // vector would not see a `/` from the previous line).
        let self_closing = !state.jsx_closing && state.jsx_slash;
        state.jsx_slash = false;
        if state.jsx_closing || self_closing {
            close_jsx_element(state);
        } else {
            state.mode = ScriptMode::JsxChildren;
        }
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    if b == b'=' {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Punctuator)?;
        state.jsx_slash = false;
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    if is_ident_start(b) || b == b'\\' {
        *i = scan_jsx_tag_name(bytes, *i);
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident, state.typescript()) {
            ScriptKind::Keyword
        } else {
            ScriptKind::Identifier
        };
        push_token(tokens, start, *i, kind)?;
        state.jsx_slash = false;
        set_prev(state, PrevSig::Ident);
        return Ok(());
    }
    *i += 1;
    push_token(tokens, start, *i, ScriptKind::Punctuator)?;
    state.jsx_slash = false;
    set_prev(state, PrevSig::Punct);
    Ok(())
}

fn resume_jsx_children(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    let _ = cancel;
    let b = bytes[*i];
    if b == b'{' {
        *i += 1;
        push_token(tokens, start, *i, ScriptKind::Delimiter)?;
        if state.push_frame(ScriptMode::JsxChildren) && state.push_braces(1) {
            state.mode = ScriptMode::JsxExpr;
        }
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    if b == b'<' {
        let mut k = *i + 1;
        k = skip_ws(bytes, k);
        if bytes.get(k) == Some(&b'/') {
            // Closing tag: switch into tag mode without pushing a frame.
            state.jsx_closing = true;
            state.mode = ScriptMode::JsxTag;
            *i += 1;
            push_token(tokens, start, *i, ScriptKind::Punctuator)?;
            set_prev(state, PrevSig::Punct);
            return Ok(());
        }
        // Nested element.
        return start_jsx(bytes, i, state, tokens, start);
    }
    // JsxText until `<` or `{` or EOF.
    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'<' || c == b'{' {
            break;
        }
        *i += 1;
    }
    if (*i as u32) > start {
        push_token(tokens, start, *i, ScriptKind::JsxText)?;
    }
    Ok(())
}

fn close_jsx_element(state: &mut ScriptState) {
    state.jsx_depth = state.jsx_depth.saturating_sub(1);
    state.jsx_closing = false;
    state.jsx_slash = false;
    let outer = state.pop_frame();
    state.mode = outer;
    set_prev(state, PrevSig::Ident);
}

fn lex_regex_or_unknown(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
) -> Result<(), LexError> {
    let mut j = *i + 1;
    let mut in_class = false;
    let mut terminated = false;
    while j < bytes.len() {
        let b = bytes[j];
        if b == b'\n' || b == b'\r' {
            break;
        }
        if b == b'\\' {
            j += 1;
            if j < bytes.len() && bytes[j] != b'\n' && bytes[j] != b'\r' {
                j += 1;
            }
            continue;
        }
        if in_class {
            if b == b']' {
                in_class = false;
            }
            j += 1;
            continue;
        }
        if b == b'[' {
            in_class = true;
            j += 1;
            continue;
        }
        if b == b'/' {
            j += 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                // flags
                j += 1;
            }
            terminated = true;
            break;
        }
        j += 1;
    }
    if !terminated {
        // Would run past the line end: do not guess. One Unknown token from
        // `/` up to (not including) the newline.
        *i = j;
        if (*i as u32) <= start {
            *i = (start as usize) + 1;
        }
        push_token(tokens, start, *i, ScriptKind::Unknown)?;
        set_prev(state, PrevSig::Punct);
        return Ok(());
    }
    *i = j;
    push_token(tokens, start, *i, ScriptKind::Regex)?;
    set_prev(state, PrevSig::String);
    Ok(())
}

fn lex_slash_punct(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ScriptState,
    tokens: &mut Vec<ScriptToken>,
    start: u32,
) -> Result<(), LexError> {
    if bytes.get(*i + 1) == Some(&b'=') {
        *i += 2;
    } else {
        *i += 1;
    }
    push_token(tokens, start, *i, ScriptKind::Punctuator)?;
    set_prev(state, PrevSig::Punct);
    Ok(())
}

fn scan_ident(bytes: &[u8], mut i: usize) -> usize {
    if i >= bytes.len() {
        return i;
    }
    if bytes[i] == b'\\' {
        i = scan_unicode_escape(bytes, i);
    } else {
        i += 1;
    }
    while i < bytes.len() {
        if is_ident_continue(bytes[i]) {
            i += 1;
        } else if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'u') {
            i = scan_unicode_escape(bytes, i);
        } else {
            break;
        }
    }
    i
}

fn scan_jsx_tag_name(bytes: &[u8], mut i: usize) -> usize {
    i = scan_ident(bytes, i);
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' || b == b'-' || b == b':' {
            if i + 1 < bytes.len() && (is_ident_start(bytes[i + 1]) || is_ident_continue(bytes[i + 1]))
            {
                i += 1;
                i = scan_ident(bytes, i);
                continue;
            }
            break;
        }
        break;
    }
    i
}

fn scan_unicode_escape(bytes: &[u8], mut i: usize) -> usize {
    // `i` at `\`. Left as-is: consume `\uXXXX` or `\u{...}` without decoding.
    if i + 1 >= bytes.len() || bytes[i + 1] != b'u' {
        return (i + 1).min(bytes.len());
    }
    i += 2;
    if bytes.get(i) == Some(&b'{') {
        i += 1;
        while i < bytes.len() && bytes[i] != b'}' {
            if !bytes[i].is_ascii_hexdigit() {
                break;
            }
            i += 1;
        }
        if bytes.get(i) == Some(&b'}') {
            i += 1;
        }
        return i;
    }
    let mut n = 0;
    while n < 4 && i < bytes.len() && bytes[i].is_ascii_hexdigit() {
        i += 1;
        n += 1;
    }
    i
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'.' {
        i += 1;
        i = scan_digits_us(bytes, i);
        i = scan_exponent(bytes, i);
        return scan_bigint_suffix(bytes, i);
    }
    if bytes[i] == b'0' {
        match bytes.get(i + 1).copied() {
            Some(b'x' | b'X') => {
                i += 2;
                i = scan_digits_base(bytes, i, is_hex);
                return scan_bigint_suffix(bytes, i);
            }
            Some(b'o' | b'O') => {
                i += 2;
                i = scan_digits_base(bytes, i, is_oct);
                return scan_bigint_suffix(bytes, i);
            }
            Some(b'b' | b'B') => {
                i += 2;
                i = scan_digits_base(bytes, i, is_bin);
                return scan_bigint_suffix(bytes, i);
            }
            _ => {}
        }
    }
    i = scan_digits_us(bytes, i);
    if bytes.get(i) == Some(&b'.')
        && bytes
            .get(i + 1)
            .copied()
            .map(|c| c.is_ascii_digit() || c == b'_')
            .unwrap_or(false)
    {
        i += 1;
        i = scan_digits_us(bytes, i);
    } else if bytes.get(i) == Some(&b'.')
        && bytes
            .get(i + 1)
            .copied()
            .map(|c| !is_ident_start(c) && c != b'.')
            .unwrap_or(true)
    {
        // trailing dot of a decimal: keep as part of the number only when
        // not `..` / ident. `1.` is a valid JS number.
        i += 1;
    }
    i = scan_exponent(bytes, i);
    scan_bigint_suffix(bytes, i)
}

fn scan_digits_us(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    i
}

fn scan_digits_base(bytes: &[u8], mut i: usize, ok: fn(u8) -> bool) -> usize {
    while i < bytes.len() && (ok(bytes[i]) || bytes[i] == b'_') {
        i += 1;
    }
    i
}

fn scan_exponent(bytes: &[u8], mut i: usize) -> usize {
    if matches!(bytes.get(i).copied(), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(bytes.get(j).copied(), Some(b'+' | b'-')) {
            j += 1;
        }
        if j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'_') {
            i = scan_digits_us(bytes, j);
        }
    }
    i
}

fn scan_bigint_suffix(bytes: &[u8], i: usize) -> usize {
    if bytes.get(i) == Some(&b'n') {
        i + 1
    } else {
        i
    }
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}
fn is_oct(b: u8) -> bool {
    (b'0'..=b'7').contains(&b)
}
fn is_bin(b: u8) -> bool {
    b == b'0' || b == b'1'
}

fn scan_punct_len(bytes: &[u8], i: usize) -> usize {
    let three = bytes.get(i..i + 3).unwrap_or(&[]);
    if matches!(
        three,
        b"===" | b"!==" | b">>>" | b"**=" | b"&&=" | b"||=" | b"??=" | b">>=" | b"<<=" | b"..."
    ) {
        return 3;
    }
    let two = bytes.get(i..i + 2).unwrap_or(&[]);
    if two == b"?." {
        // `?.` is optional chaining unless the next byte is a digit (`? .5`).
        if bytes.get(i + 2).copied().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return 1;
        }
        return 2;
    }
    if matches!(
        two,
        b"==" | b"!="
            | b"<="
            | b">="
            | b"<<"
            | b">>"
            | b"&&"
            | b"||"
            | b"??"
            | b"=>"
            | b"++"
            | b"--"
            | b"+="
            | b"-="
            | b"*="
            | b"/="
            | b"%="
            | b"&="
            | b"|="
            | b"^="
            | b"**"
    ) {
        return 2;
    }
    1
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (is_space(bytes[i]) || bytes[i] == b'\n' || bytes[i] == b'\r') {
        i += 1;
    }
    i
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

fn prev_of(state: &ScriptState) -> PrevSig {
    match state.prev {
        1 => PrevSig::Close,
        2 => PrevSig::Punct,
        3 => PrevSig::Ident,
        4 => PrevSig::Number,
        5 => PrevSig::String,
        6 => PrevSig::RegexKw,
        _ => PrevSig::None,
    }
}

fn set_prev(state: &mut ScriptState, p: PrevSig) {
    state.prev = p as u8;
}

fn push_token(
    tokens: &mut Vec<ScriptToken>,
    start: u32,
    end: usize,
    kind: ScriptKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(ScriptToken {
        start,
        end: end as u32,
        kind,
    });
    Ok(())
}

fn push_role(end: usize, role: TokenRole, out: &mut Vec<(usize, TokenRole)>) {
    if end == 0 {
        return;
    }
    if out.last().map(|(_, r)| *r) == Some(role) {
        out.last_mut().unwrap().0 = end;
    } else {
        out.push((end, role));
    }
}
