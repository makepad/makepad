//! Source-only Haskell lexer shared by the editor and the Haskell frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: nested `{- -}` comments, string gaps, quasiquotes and literate
//! Bird/LaTeX line rules are not packed into a universal integer.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const HASKELL_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum HaskellMode {
    #[default]
    Normal,
    BlockComment {
        depth: u8,
        pragma: bool,
    },
    String {
        /// After `\` at end of line: next line is a string gap.
        gap: bool,
    },
    Quasi,
}

/// Provider-owned line continuation. Default is Normal (not inside a
/// comment, string or quasiquote). `lex_line` and the document lexer share
/// one state machine so highlighting and parsing agree.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct HaskellState {
    mode: HaskellMode,
    /// `.lhs` Bird literate: lines that do not start with `>` are prose.
    literate: bool,
    /// Inside a `\begin{code}` … `\end{code}` LaTeX literate chunk.
    latex_code: bool,
}

impl HaskellState {
    pub fn literate() -> Self {
        HaskellState {
            literate: true,
            ..Self::default()
        }
    }

    pub fn is_literate(&self) -> bool {
        self.literate
    }

    pub fn in_latex_code(&self) -> bool {
        self.latex_code
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HaskellKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Keyword,
    Number,
    String,
    Char,
    Punctuator,
    Delimiter,
    Preprocessor,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HaskellToken {
    pub start: u32,
    pub end: u32,
    pub kind: HaskellKind,
}

impl HaskellKind {
    pub fn role(self) -> TokenRole {
        match self {
            HaskellKind::Whitespace | HaskellKind::Newline => TokenRole::Whitespace,
            HaskellKind::Comment => TokenRole::Comment,
            HaskellKind::Identifier => TokenRole::Identifier,
            HaskellKind::Keyword => TokenRole::Keyword,
            HaskellKind::Number => TokenRole::Number,
            HaskellKind::String => TokenRole::String,
            HaskellKind::Char => TokenRole::Char,
            HaskellKind::Punctuator => TokenRole::Punctuator,
            HaskellKind::Delimiter => TokenRole::Delimiter,
            HaskellKind::Preprocessor => TokenRole::Preprocessor,
            HaskellKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// Haskell 2010 reserved identifiers, sorted for binary search.
const KEYWORDS: &[&str] = &[
    "case", "class", "data", "default", "deriving", "do", "else", "foreign", "if", "import", "in",
    "infix", "infixl", "infixr", "instance", "let", "module", "newtype", "of", "then", "type",
    "where",
];

pub fn is_keyword(ident: &str) -> bool {
    ident == "_" || KEYWORDS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_paren: bool) -> TokenRole {
    match ident {
        "if" | "then" | "else" | "case" | "of" => TokenRole::BranchKeyword,
        "do" => TokenRole::LoopKeyword,
        other if is_keyword(other) => TokenRole::Keyword,
        _ if next_is_paren => TokenRole::Function,
        other if other
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false) =>
        {
            TokenRole::Typename
        }
        _ => TokenRole::Identifier,
    }
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' || b >= 0x80
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn is_symbol(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'*'
            | b'+'
            | b'.'
            | b'/'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'\\'
            | b'^'
            | b'|'
            | b'-'
            | b'~'
            | b':'
    )
}

fn push_token(
    tokens: &mut Vec<HaskellToken>,
    start: u32,
    end: usize,
    kind: HaskellKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(HaskellToken {
        start,
        end: end as u32,
        kind,
    });
    Ok(())
}

fn consume_newline(bytes: &[u8], i: &mut usize) {
    if bytes.get(*i) == Some(&b'\r') {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
    } else if bytes.get(*i) == Some(&b'\n') {
        *i += 1;
    }
}

fn starts_with_at(bytes: &[u8], i: usize, needle: &[u8]) -> bool {
    bytes.get(i..).is_some_and(|rest| rest.starts_with(needle))
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0' {
        let tag = bytes.get(i + 1).map(|b| b.to_ascii_lowercase());
        if matches!(tag, Some(b'x' | b'o' | b'b')) {
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                let ok = match tag {
                    Some(b'x') => b.is_ascii_hexdigit() || b == b'_',
                    Some(b'o') => (b'0'..=b'7').contains(&b) || b == b'_',
                    Some(b'b') => b == b'0' || b == b'1' || b == b'_',
                    _ => false,
                };
                if !ok {
                    break;
                }
                i += 1;
            }
            return i;
        }
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        if next == b'.' {
            // `1..10`: leave `..` for the operator scanner.
        } else if next.is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < bytes.len() && matches!(bytes[j], b'+' | b'-') {
            j += 1;
        }
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            i = j + 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    i
}

fn dash_run_is_comment(bytes: &[u8], i: usize) -> bool {
    let mut j = i;
    while j < bytes.len() && bytes[j] == b'-' {
        j += 1;
    }
    if j - i < 2 {
        return false;
    }
    // `--` followed by a symbol is a legal operator (`-->`), not a comment.
    bytes.get(j).copied().map(|b| !is_symbol(b)).unwrap_or(true)
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HaskellState,
    tokens: &mut Vec<HaskellToken>,
    line_start: &mut bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    if *i >= bytes.len() {
        return Ok(());
    }
    let start = *i as u32;

    match state.mode {
        HaskellMode::BlockComment { depth, pragma } => {
            return resume_block_comment(bytes, i, state, tokens, start, depth, pragma, line_start);
        }
        HaskellMode::String { gap } => {
            return resume_string(bytes, i, state, tokens, start, gap, line_start);
        }
        HaskellMode::Quasi => {
            return resume_quasi(bytes, i, state, tokens, start, line_start, cancel);
        }
        HaskellMode::Normal => {}
    }

    if bytes[*i..].starts_with(&[0xef, 0xbb, 0xbf]) {
        *i += 3;
        push_token(tokens, start, *i, HaskellKind::Whitespace)?;
        return Ok(());
    }

    let b = bytes[*i];

    if b == b'\r' || b == b'\n' {
        consume_newline(bytes, i);
        *line_start = true;
        push_token(tokens, start, *i, HaskellKind::Newline)?;
        return Ok(());
    }

    if *line_start && state.literate && !state.latex_code {
        if b == b'>' {
            *i += 1;
            *line_start = false;
            push_token(tokens, start, *i, HaskellKind::Punctuator)?;
            return Ok(());
        }
        if starts_with_at(bytes, *i, b"\\begin{code}") {
            *i += b"\\begin{code}".len();
            state.latex_code = true;
            *line_start = false;
            push_token(tokens, start, *i, HaskellKind::Preprocessor)?;
            return Ok(());
        }
        // Bird prose: the rest of the line is documentation.
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        push_token(tokens, start, *i, HaskellKind::Comment)?;
        return Ok(());
    }

    if *line_start && state.latex_code && starts_with_at(bytes, *i, b"\\end{code}") {
        *i += b"\\end{code}".len();
        state.latex_code = false;
        *line_start = false;
        push_token(tokens, start, *i, HaskellKind::Preprocessor)?;
        return Ok(());
    }

    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        push_token(tokens, start, *i, HaskellKind::Whitespace)?;
        return Ok(());
    }

    if *line_start && b == b'#' {
        *i += 1;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        *line_start = false;
        push_token(tokens, start, *i, HaskellKind::Preprocessor)?;
        return Ok(());
    }

    *line_start = false;

    if b == b'{' && bytes.get(*i + 1) == Some(&b'-') {
        let pragma = bytes.get(*i + 2) == Some(&b'#');
        *i += if pragma { 3 } else { 2 };
        state.mode = HaskellMode::BlockComment { depth: 1, pragma };
        return resume_block_comment(bytes, i, state, tokens, start, 1, pragma, line_start);
    }

    if b == b'-' && dash_run_is_comment(bytes, *i) {
        while *i < bytes.len() && bytes[*i] == b'-' {
            *i += 1;
        }
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            *i += 1;
        }
        push_token(tokens, start, *i, HaskellKind::Comment)?;
        return Ok(());
    }

    if b == b'"' {
        *i += 1;
        state.mode = HaskellMode::String { gap: false };
        return resume_string(bytes, i, state, tokens, start, false, line_start);
    }

    if b == b'\'' {
        return lex_char(bytes, i, tokens, start);
    }

    if b == b'[' && looks_like_quasi(bytes, *i) {
        *i = skip_quasi_open(bytes, *i);
        state.mode = HaskellMode::Quasi;
        return resume_quasi(bytes, i, state, tokens, start, line_start, cancel);
    }

    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            HaskellKind::Keyword
        } else {
            HaskellKind::Identifier
        };
        push_token(tokens, start, *i, kind)?;
        return Ok(());
    }

    if b.is_ascii_digit() {
        *i = scan_number(bytes, *i);
        push_token(tokens, start, *i, HaskellKind::Number)?;
        return Ok(());
    }

    if matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']' | b',' | b';') {
        *i += 1;
        let kind = if matches!(b, b',' | b';') {
            HaskellKind::Punctuator
        } else {
            HaskellKind::Delimiter
        };
        push_token(tokens, start, *i, kind)?;
        return Ok(());
    }

    if b == b'`' {
        *i += 1;
        push_token(tokens, start, *i, HaskellKind::Punctuator)?;
        return Ok(());
    }

    if is_symbol(b) {
        *i += 1;
        while *i < bytes.len() && is_symbol(bytes[*i]) {
            *i += 1;
        }
        push_token(tokens, start, *i, HaskellKind::Punctuator)?;
        return Ok(());
    }

    *i += 1;
    if (*i as u32) <= start {
        *i = (start as usize) + 1;
    }
    let kind = if bytes[start as usize].is_ascii_graphic() {
        HaskellKind::Punctuator
    } else {
        HaskellKind::Unknown
    };
    push_token(tokens, start, *i, kind)?;
    Ok(())
}

fn looks_like_quasi(bytes: &[u8], i: usize) -> bool {
    // `[|`, `[e|`, `[$e|` — quasiquote openers. `[1,2]` is a list.
    let rest = &bytes[i..];
    if rest.len() < 2 {
        return false;
    }
    if rest[1] == b'|' {
        return true;
    }
    let mut j = 1;
    if rest.get(j) == Some(&b'$') {
        j += 1;
    }
    if j >= rest.len() || !is_ident_start(rest[j]) {
        return false;
    }
    j += 1;
    while j < rest.len() && is_ident_continue(rest[j]) {
        j += 1;
    }
    rest.get(j) == Some(&b'|')
}

fn skip_quasi_open(bytes: &[u8], mut i: usize) -> usize {
    // Consume `[` `$`? ident `|`.
    i += 1;
    if bytes.get(i) == Some(&b'|') {
        return i + 1;
    }
    if bytes.get(i) == Some(&b'$') {
        i += 1;
    }
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }
    if bytes.get(i) == Some(&b'|') {
        i += 1;
    }
    i
}

fn resume_block_comment(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HaskellState,
    tokens: &mut Vec<HaskellToken>,
    start: u32,
    mut depth: u8,
    pragma: bool,
    line_start: &mut bool,
) -> Result<(), LexError> {
    let kind = if pragma {
        HaskellKind::Preprocessor
    } else {
        HaskellKind::Comment
    };
    while *i < bytes.len() && depth > 0 {
        if bytes[*i] == b'{' && bytes.get(*i + 1) == Some(&b'-') {
            *i += 2;
            depth = depth.saturating_add(1);
            continue;
        }
        if pragma && bytes[*i] == b'#' && bytes.get(*i + 1) == Some(&b'-') && bytes.get(*i + 2) == Some(&b'}')
        {
            *i += 3;
            depth = depth.saturating_sub(1);
            continue;
        }
        if bytes[*i] == b'-' && bytes.get(*i + 1) == Some(&b'}') {
            *i += 2;
            depth = depth.saturating_sub(1);
            continue;
        }
        if is_newline(bytes[*i]) {
            if *i as u32 > start {
                state.mode = HaskellMode::BlockComment { depth, pragma };
                push_token(tokens, start, *i, kind)?;
                return Ok(());
            }
            consume_newline(bytes, i);
            *line_start = true;
            push_token(tokens, start, *i, HaskellKind::Newline)?;
            state.mode = HaskellMode::BlockComment { depth, pragma };
            return Ok(());
        }
        *i += 1;
    }
    if depth == 0 {
        state.mode = HaskellMode::Normal;
    } else {
        state.mode = HaskellMode::BlockComment { depth, pragma };
    }
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    push_token(tokens, start, *i, kind)?;
    Ok(())
}

fn resume_string(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HaskellState,
    tokens: &mut Vec<HaskellToken>,
    start: u32,
    mut gap: bool,
    line_start: &mut bool,
) -> Result<(), LexError> {
    if gap {
        while *i < bytes.len() && (is_space(bytes[*i]) || is_newline(bytes[*i])) {
            if is_newline(bytes[*i]) {
                if *i as u32 > start {
                    state.mode = HaskellMode::String { gap: true };
                    push_token(tokens, start, *i, HaskellKind::String)?;
                    return Ok(());
                }
                consume_newline(bytes, i);
                *line_start = true;
                push_token(tokens, start, *i, HaskellKind::Newline)?;
                state.mode = HaskellMode::String { gap: true };
                return Ok(());
            }
            *i += 1;
        }
        if bytes.get(*i) == Some(&b'\\') {
            *i += 1;
            gap = false;
        } else if *i >= bytes.len() {
            state.mode = HaskellMode::String { gap: true };
            if (*i as u32) > start {
                push_token(tokens, start, *i, HaskellKind::String)?;
            }
            return Ok(());
        } else {
            gap = false;
        }
    }
    while *i < bytes.len() {
        let b = bytes[*i];
        if is_newline(b) {
            state.mode = HaskellMode::String { gap };
            if *i as u32 > start {
                push_token(tokens, start, *i, HaskellKind::String)?;
                return Ok(());
            }
            consume_newline(bytes, i);
            *line_start = true;
            push_token(tokens, start, *i, HaskellKind::Newline)?;
            return Ok(());
        }
        if b == b'\\' {
            *i += 1;
            if *i >= bytes.len() || is_newline(bytes[*i]) || is_space(bytes[*i]) {
                // Gap: `\` then whitespace (possibly a following newline).
                while *i < bytes.len() && is_space(bytes[*i]) {
                    *i += 1;
                }
                if *i >= bytes.len() || is_newline(bytes[*i]) {
                    state.mode = HaskellMode::String { gap: true };
                    if (*i as u32) > start {
                        push_token(tokens, start, *i, HaskellKind::String)?;
                    } else {
                        return Err(LexError::Nonprogress { at: start });
                    }
                    return Ok(());
                }
                if bytes.get(*i) == Some(&b'\\') {
                    *i += 1;
                    continue;
                }
                continue;
            }
            *i += 1;
            continue;
        }
        if b == b'"' {
            *i += 1;
            state.mode = HaskellMode::Normal;
            push_token(tokens, start, *i, HaskellKind::String)?;
            return Ok(());
        }
        *i += 1;
    }
    state.mode = HaskellMode::String { gap: false };
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    push_token(tokens, start, *i, HaskellKind::String)?;
    Ok(())
}

fn resume_quasi(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HaskellState,
    tokens: &mut Vec<HaskellToken>,
    start: u32,
    line_start: &mut bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    while *i < bytes.len() {
        if *i & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if is_newline(bytes[*i]) {
            if *i as u32 > start {
                state.mode = HaskellMode::Quasi;
                push_token(tokens, start, *i, HaskellKind::String)?;
                return Ok(());
            }
            consume_newline(bytes, i);
            *line_start = true;
            push_token(tokens, start, *i, HaskellKind::Newline)?;
            state.mode = HaskellMode::Quasi;
            return Ok(());
        }
        if bytes[*i] == b'|' && bytes.get(*i + 1) == Some(&b']') {
            *i += 2;
            state.mode = HaskellMode::Normal;
            push_token(tokens, start, *i, HaskellKind::String)?;
            return Ok(());
        }
        *i += 1;
    }
    state.mode = HaskellMode::Quasi;
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    push_token(tokens, start, *i, HaskellKind::String)?;
    Ok(())
}

fn lex_char(
    bytes: &[u8],
    i: &mut usize,
    tokens: &mut Vec<HaskellToken>,
    start: u32,
) -> Result<(), LexError> {
    // `'` at `*i`. Type variables (`'a`) are a single `'` plus ident;
    // character literals are `'x'` or `'\\n'`.
    let next = bytes.get(*i + 1).copied();
    if next == Some(b'\\') {
        *i += 2;
        if *i < bytes.len() && !is_newline(bytes[*i]) {
            if bytes[*i] == b'x' || bytes[*i] == b'o' || bytes[*i].is_ascii_digit() {
                *i += 1;
                while *i < bytes.len() && (bytes[*i].is_ascii_hexdigit() || bytes[*i].is_ascii_digit())
                {
                    *i += 1;
                }
            } else if bytes[*i] == b'^' {
                *i += 1;
                if *i < bytes.len() && !is_newline(bytes[*i]) {
                    *i += 1;
                }
            } else {
                *i += 1;
            }
        }
        if bytes.get(*i) == Some(&b'\'') {
            *i += 1;
            push_token(tokens, start, *i, HaskellKind::Char)?;
            return Ok(());
        }
        if (*i as u32) <= start {
            *i = (start as usize) + 1;
        }
        push_token(tokens, start, *i, HaskellKind::Char)?;
        return Ok(());
    }
    if next.is_some_and(is_ident_start) {
        let third = bytes.get(*i + 2).copied();
        if third == Some(b'\'') {
            *i += 3;
            push_token(tokens, start, *i, HaskellKind::Char)?;
            return Ok(());
        }
        // `'a` type variable.
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        push_token(tokens, start, *i, HaskellKind::Identifier)?;
        return Ok(());
    }
    *i += 1;
    if *i < bytes.len() && !is_newline(bytes[*i]) && bytes[*i] != b'\'' {
        *i += 1;
    }
    if bytes.get(*i) == Some(&b'\'') {
        *i += 1;
        push_token(tokens, start, *i, HaskellKind::Char)?;
        return Ok(());
    }
    if (*i as u32) <= start {
        *i = (start as usize) + 1;
    }
    push_token(tokens, start, *i, HaskellKind::Unknown)?;
    Ok(())
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
/// Every byte is covered by exactly one token. `cancel` is observed every
/// 256 tokens. A run that does not advance the cursor is `Nonprogress`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<HaskellToken>, LexError> {
    lex_document_from(bytes, HaskellState::default(), cancel)
}

/// Tokenize with an explicit initial continuation (literate Bird dialect).
pub fn lex_document_from(
    bytes: &[u8],
    mut state: HaskellState,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<HaskellToken>, LexError> {
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut line_start = true;
    let token_cap = bytes.len().saturating_add(8);
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if tokens.len() > token_cap {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
        let start_i = i;
        lex_one(
            bytes,
            &mut i,
            &mut state,
            &mut tokens,
            &mut line_start,
            cancel,
        )?;
        if i == start_i && i < bytes.len() {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
    }
    Ok(tokens)
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: HaskellState) -> (HaskellState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = matches!(state.mode, HaskellMode::Normal);
    if bytes.is_empty() {
        return (state, Vec::new());
    }
    let cancel = || false;
    while i < bytes.len() {
        let start_i = i;
        if lex_one(
            bytes,
            &mut i,
            &mut state,
            &mut tokens,
            &mut line_start,
            &cancel,
        )
        .is_err()
        {
            if i == start_i {
                i += 1;
                let _ = push_token(&mut tokens, start_i as u32, i, HaskellKind::Unknown);
            }
        }
        if i == start_i && i < bytes.len() {
            i += 1;
            let _ = push_token(&mut tokens, start_i as u32, i, HaskellKind::Unknown);
        }
    }
    let roles = tokens_to_roles(bytes, &tokens);
    (state, roles)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead. `(` after a newline is not a
/// Function role on the preceding identifier.
pub fn document_spans(bytes: &[u8], tokens: &[HaskellToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == HaskellKind::Keyword || t.kind == HaskellKind::Identifier {
            let mut j = i + 1;
            let mut next_is_paren = false;
            while j < tokens.len() {
                match tokens[j].kind {
                    HaskellKind::Newline => break,
                    HaskellKind::Whitespace | HaskellKind::Comment => j += 1,
                    HaskellKind::Delimiter
                        if bytes.get(tokens[j].start as usize) == Some(&b'(') =>
                    {
                        next_is_paren = true;
                        break;
                    }
                    _ => break,
                }
            }
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}

fn tokens_to_roles(bytes: &[u8], tokens: &[HaskellToken]) -> Vec<(usize, TokenRole)> {
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == HaskellKind::Keyword || t.kind == HaskellKind::Identifier {
            let mut j = i + 1;
            let mut next_is_paren = false;
            while j < tokens.len() {
                match tokens[j].kind {
                    HaskellKind::Newline => break,
                    HaskellKind::Whitespace | HaskellKind::Comment => j += 1,
                    HaskellKind::Delimiter
                        if bytes.get(tokens[j].start as usize) == Some(&b'(') =>
                    {
                        next_is_paren = true;
                        break;
                    }
                    _ => break,
                }
            }
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren);
        }
        push_role(t.end as usize, role, &mut out);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::detect_path;
    use crate::id::{DetectionSource, LanguageId};
    use crate::token::TokenRole;

    fn roles_of(line: &str) -> Vec<(String, TokenRole)> {
        let (_s, ends) = lex_line(line, HaskellState::default());
        let mut out = Vec::new();
        let mut at = 0usize;
        for (end, role) in ends {
            out.push((line[at..end].to_string(), role));
            at = end;
        }
        out
    }

    fn kinds(src: &str) -> Vec<(String, HaskellKind)> {
        let tokens = lex_document_cancellable(src.as_bytes(), &|| false).unwrap();
        tokens
            .iter()
            .map(|t| {
                (
                    src[t.start as usize..t.end as usize].to_string(),
                    t.kind,
                )
            })
            .collect()
    }

    #[test]
    fn detects_hs_and_lhs() {
        let hs = detect_path("src/Main.hs", None);
        assert_eq!(hs.language, LanguageId::Haskell);
        assert_eq!(hs.dialect.name, "");
        assert_eq!(hs.source, DetectionSource::Extension);
        let lhs = detect_path("Doc.lhs", None);
        assert_eq!(lhs.language, LanguageId::Haskell);
        assert_eq!(lhs.dialect.name, "literate");
        assert!(crate::detect::inventory_extensions().contains(&"hs"));
        assert!(crate::detect::inventory_extensions().contains(&"lhs"));
        assert!(crate::detect::has_compiled_frontend(LanguageId::Haskell));
        assert_eq!(LanguageId::Haskell as u16, 29);
        assert_eq!(LanguageId::Zig as u16, 28);
    }

    #[test]
    fn nested_comment_continues_across_lines() {
        let (s1, _) = lex_line("foo {- bar", HaskellState::default());
        assert!(matches!(
            s1.mode,
            HaskellMode::BlockComment { depth: 1, pragma: false }
        ));
        let (s2, roles) = lex_line(" baz -} x", s1);
        assert_eq!(s2.mode, HaskellMode::Normal);
        assert!(roles.iter().any(|(_, r)| *r == TokenRole::Comment));
        assert!(roles.iter().any(|(_, r)| *r == TokenRole::Identifier));
    }

    #[test]
    fn pragma_is_preprocessor() {
        let k = kinds("{-# LANGUAGE GADTs #-}");
        assert!(k.iter().any(|(_, kind)| *kind == HaskellKind::Preprocessor));
    }

    #[test]
    fn dash_operator_is_not_comment() {
        let k = kinds("x --> y");
        assert!(k.iter().any(|(t, kind)| t == "-->" && *kind == HaskellKind::Punctuator));
        assert!(!k.iter().any(|(_, kind)| *kind == HaskellKind::Comment));
    }

    #[test]
    fn keyword_and_typename_roles() {
        let r = roles_of("data Maybe a = Just a | Nothing");
        assert!(r.iter().any(|(t, role)| t == "data" && *role == TokenRole::Keyword));
        assert!(r
            .iter()
            .any(|(t, role)| t == "Maybe" && *role == TokenRole::Typename));
        assert!(r.iter().any(|(t, _role)| t == "Just"));
        let r = roles_of("if x then y else z");
        assert!(r
            .iter()
            .any(|(t, role)| t == "if" && *role == TokenRole::BranchKeyword));
    }

    #[test]
    fn string_gap_continues() {
        let (s1, _) = lex_line("\"foo\\", HaskellState::default());
        assert!(matches!(s1.mode, HaskellMode::String { gap: true }));
        let (s2, roles) = lex_line("  \\bar\"", s1);
        assert_eq!(s2.mode, HaskellMode::Normal);
        assert!(roles.iter().any(|(_, r)| *r == TokenRole::String));
    }

    #[test]
    fn bird_literate_code_vs_prose() {
        let src = "This is prose\n> module Foo where\n";
        let tokens =
            lex_document_from(src.as_bytes(), HaskellState::literate(), &|| false).unwrap();
        let prose = tokens
            .iter()
            .find(|t| t.kind == HaskellKind::Comment)
            .map(|t| &src[t.start as usize..t.end as usize])
            .unwrap();
        assert!(prose.contains("prose"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == HaskellKind::Keyword
                && &src[t.start as usize..t.end as usize] == "module"));
        let (s, roles) = lex_line("This is prose", HaskellState::literate());
        assert_eq!(s.mode, HaskellMode::Normal);
        assert!(roles.iter().all(|(_, r)| *r == TokenRole::Comment));
        let (_s, roles) = lex_line("> foo = 1", HaskellState::literate());
        assert!(roles.iter().any(|(_, r)| *r == TokenRole::Identifier));
    }

    #[test]
    fn latex_chunk_markers() {
        let src = "\\begin{code}\nx = 1\n\\end{code}\n";
        let tokens =
            lex_document_from(src.as_bytes(), HaskellState::literate(), &|| false).unwrap();
        assert!(tokens
            .iter()
            .any(|t| t.kind == HaskellKind::Preprocessor
                && src[t.start as usize..t.end as usize].contains("begin")));
        assert!(tokens
            .iter()
            .any(|t| t.kind == HaskellKind::Preprocessor
                && src[t.start as usize..t.end as usize].contains("end")));
    }

    #[test]
    fn quasiquote_spans_as_string() {
        let k = kinds("[e| hello |] rest");
        assert!(k.iter().any(|(_, kind)| *kind == HaskellKind::String));
        assert!(k
            .iter()
            .any(|(t, kind)| t == "rest" && *kind == HaskellKind::Identifier));
    }
}
