//! Source-only POSIX sh / Bash / Zsh lexer shared by the editor and the text frontend.
//!
//! Tokens only. No expansion, globbing, command or heredoc semantics. Source is
//! never executed. Malformed input is classified, never rejected.

use crate::cpp_lex::LexError;
use crate::id::Dialect;
use crate::token::{TokenRole, TokenSpan};

/// Version of this lexer; part of parse and search cache identity.
pub const SHELL_LEXER_VERSION: u32 = 1;

const HEREDOC_TAG_CAP: usize = 24;
const NEST_CAP: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ShellDialect {
    #[default]
    Posix,
    Bash,
    Zsh,
}

impl ShellDialect {
    pub fn from_dialect(d: Dialect) -> Self {
        match d.name {
            "bash" => ShellDialect::Bash,
            "zsh" => ShellDialect::Zsh,
            _ => ShellDialect::Posix,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum ShellMode {
    #[default]
    Normal,
    Single,
    DollarSingle,
    Double,
    Heredoc,
}

/// Nest frame: command substitution, arithmetic, backtick.
/// `kind`: 1 = `$(`, 2 = `$((`, 3 = backtick, 4 = `$(` from double quotes.
const NEST_CMD: u8 = 1;
const NEST_ARITH: u8 = 2;
const NEST_TICK: u8 = 3;
const NEST_DBL: u8 = 4;
const NEST_DBLARITH: u8 = 5;

/// Provider-owned line continuation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShellState {
    dialect: ShellDialect,
    mode: ShellMode,
    nest_kind: [u8; 4],
    nest_paren: [u8; 4],
    nest_len: u8,
    at_command: bool,
    line_continued: bool,
    restore_command: bool,
    pending_heredoc: bool,
    heredoc_strip_tabs: bool,
    heredoc_tag: [u8; HEREDOC_TAG_CAP],
    heredoc_tag_len: u8,
}

impl Default for ShellState {
    fn default() -> Self {
        ShellState::for_dialect(ShellDialect::Posix)
    }
}

impl ShellState {
    pub fn for_dialect(dialect: ShellDialect) -> Self {
        ShellState {
            dialect,
            mode: ShellMode::Normal,
            nest_kind: [0; 4],
            nest_paren: [0; 4],
            nest_len: 0,
            at_command: true,
            line_continued: false,
            restore_command: false,
            pending_heredoc: false,
            heredoc_strip_tabs: false,
            heredoc_tag: [0; HEREDOC_TAG_CAP],
            heredoc_tag_len: 0,
        }
    }

    pub fn dialect(&self) -> ShellDialect {
        self.dialect
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    Whitespace,
    Comment,
    Identifier,
    Keyword,
    BranchKeyword,
    LoopKeyword,
    Function,
    Number,
    String,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellToken {
    pub start: u32,
    pub end: u32,
    pub kind: ShellKind,
}

impl ShellKind {
    pub fn role(self) -> TokenRole {
        match self {
            ShellKind::Whitespace => TokenRole::Whitespace,
            ShellKind::Comment => TokenRole::Comment,
            ShellKind::Identifier => TokenRole::Identifier,
            ShellKind::Keyword => TokenRole::Keyword,
            ShellKind::BranchKeyword => TokenRole::BranchKeyword,
            ShellKind::LoopKeyword => TokenRole::LoopKeyword,
            ShellKind::Function => TokenRole::Function,
            ShellKind::Number => TokenRole::Number,
            ShellKind::String => TokenRole::String,
            ShellKind::Punctuator => TokenRole::Punctuator,
            ShellKind::Delimiter => TokenRole::Delimiter,
            ShellKind::Unknown => TokenRole::Unknown,
        }
    }
}

fn tok(start: u32, end: usize, kind: ShellKind) -> ShellToken {
    ShellToken {
        start,
        end: end as u32,
        kind,
    }
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
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

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn comment_at_word_start(prev: Option<u8>, at_sol: bool) -> bool {
    if at_sol {
        return true;
    }
    match prev {
        None => true,
        Some(b) => is_space(b) || matches!(b, b';' | b'&' | b'|' | b'(' | b'{'),
    }
}

fn classify_command_word(word: &str) -> Option<ShellKind> {
    match word {
        "if" | "then" | "else" | "elif" | "fi" | "case" | "esac" | "select" | "return" | "exit" => {
            Some(ShellKind::BranchKeyword)
        }
        "for" | "while" | "until" | "do" | "done" | "in" | "break" | "continue" => {
            Some(ShellKind::LoopKeyword)
        }
        "function" | "time" | "coproc" | "local" | "export" | "readonly" | "declare" | "typeset"
        | "shift" | "source" | "eval" | "exec" | "set" | "unset" | "trap" | "alias" | "unalias"
        | "let" | "test" | "." => Some(ShellKind::Keyword),
        _ => None,
    }
}

fn keeps_command_position(word: &str) -> bool {
    matches!(
        word,
        "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "time" | "function"
    )
}

fn classify_plain_word(word: &[u8], at_command: bool) -> ShellKind {
    if word.is_empty() {
        return ShellKind::Unknown;
    }
    if let Ok(s) = std::str::from_utf8(word) {
        if at_command {
            if let Some(k) = classify_command_word(s) {
                return k;
            }
        }
    }
    if word.iter().all(|b| b.is_ascii_digit()) {
        return ShellKind::Number;
    }
    if word[0] == b'-' {
        return ShellKind::Punctuator;
    }
    if is_ident_start(word[0]) && word.iter().skip(1).all(|&b| is_ident_continue(b)) {
        if at_command {
            return ShellKind::Function;
        }
        return ShellKind::Identifier;
    }
    ShellKind::Unknown
}

fn word_end_byte(b: u8) -> bool {
    is_space(b)
        || is_newline(b)
        || matches!(
            b,
            b';' | b'&'
                | b'|'
                | b'('
                | b')'
                | b'{'
                | b'}'
                | b'['
                | b']'
                | b'<'
                | b'>'
                | b'"'
                | b'\''
                | b'`'
                | b'$'
                | b'#'
                | b'='
                | b'!'
                | b'~'
                | b'*'
                | b'?'
                | b'\\'
        )
}

fn skip_spaces(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && is_space(bytes[*i]) {
        *i += 1;
    }
}

fn push_nest(state: &mut ShellState, kind: u8, parens: u8) -> bool {
    if state.nest_len >= NEST_CAP {
        return false;
    }
    let n = state.nest_len as usize;
    state.nest_kind[n] = kind;
    state.nest_paren[n] = parens;
    state.nest_len += 1;
    true
}

fn pop_nest(state: &mut ShellState) -> Option<u8> {
    if state.nest_len == 0 {
        return None;
    }
    state.nest_len -= 1;
    let kind = state.nest_kind[state.nest_len as usize];
    state.nest_kind[state.nest_len as usize] = 0;
    state.nest_paren[state.nest_len as usize] = 0;
    Some(kind)
}

fn nest_top(state: &ShellState) -> Option<(u8, u8)> {
    if state.nest_len == 0 {
        return None;
    }
    let n = (state.nest_len - 1) as usize;
    Some((state.nest_kind[n], state.nest_paren[n]))
}

fn nest_paren_add(state: &mut ShellState, delta: i8) {
    if state.nest_len == 0 {
        return;
    }
    let n = (state.nest_len - 1) as usize;
    if delta > 0 {
        state.nest_paren[n] = state.nest_paren[n].saturating_add(delta as u8);
    } else {
        state.nest_paren[n] = state.nest_paren[n].saturating_sub((-delta) as u8);
    }
}

fn scan_dollar_ident(bytes: &[u8], i: &mut usize) {
    // Caller has consumed `$`.
    let Some(&c) = bytes.get(*i) else {
        return;
    };
    match c {
        b'{' => {
            *i += 1;
            let mut depth = 1u8;
            while *i < bytes.len() && depth > 0 {
                if is_newline(bytes[*i]) {
                    break;
                }
                match bytes[*i] {
                    b'{' => depth = depth.saturating_add(1),
                    b'}' => depth -= 1,
                    _ => {}
                }
                *i += 1;
            }
        }
        b'?' | b'$' | b'!' | b'#' | b'@' | b'*' | b'-' => *i += 1,
        b'0'..=b'9' => *i += 1,
        _ if is_ident_start(c) => {
            *i += 1;
            while *i < bytes.len() && is_ident_continue(bytes[*i]) {
                *i += 1;
            }
        }
        _ => {}
    }
}

fn parse_heredoc_tag(bytes: &[u8], i: &mut usize, state: &mut ShellState, strip_tabs: bool) {
    skip_spaces(bytes, i);
    if *i >= bytes.len() || is_newline(bytes[*i]) {
        return;
    }
    let mut tag = [0u8; HEREDOC_TAG_CAP];
    let mut len = 0usize;
    let c = bytes[*i];
    if c == b'\'' || c == b'"' {
        let q = c;
        *i += 1;
        while *i < bytes.len() && bytes[*i] != q && !is_newline(bytes[*i]) {
            if len < HEREDOC_TAG_CAP {
                tag[len] = bytes[*i];
                len += 1;
            }
            *i += 1;
        }
        if bytes.get(*i) == Some(&q) {
            *i += 1;
        }
    } else if c == b'\\' {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            if len < HEREDOC_TAG_CAP {
                tag[len] = bytes[*i];
                len += 1;
            }
            *i += 1;
        }
    } else {
        while *i < bytes.len() {
            let b = bytes[*i];
            if is_space(b) || is_newline(b) || matches!(b, b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')') {
                break;
            }
            if len < HEREDOC_TAG_CAP {
                tag[len] = b;
                len += 1;
            }
            *i += 1;
            if len == HEREDOC_TAG_CAP {
                break;
            }
        }
    }
    if len == 0 || state.pending_heredoc || state.mode == ShellMode::Heredoc {
        return;
    }
    state.pending_heredoc = true;
    state.heredoc_strip_tabs = strip_tabs;
    state.heredoc_tag = tag;
    state.heredoc_tag_len = len as u8;
}

fn heredoc_line_matches(bytes: &[u8], start: usize, state: &ShellState) -> Option<usize> {
    let mut i = start;
    if state.heredoc_strip_tabs {
        while bytes.get(i) == Some(&b'\t') {
            i += 1;
        }
    }
    let tag = &state.heredoc_tag[..state.heredoc_tag_len as usize];
    if bytes.get(i..).map(|s| s.starts_with(tag)).unwrap_or(false) {
        let after = i + tag.len();
        if after == bytes.len() || is_newline(bytes.get(after).copied().unwrap_or(0)) {
            return Some(after);
        }
    }
    None
}

fn clear_heredoc(state: &mut ShellState) {
    state.pending_heredoc = false;
    state.heredoc_strip_tabs = false;
    state.heredoc_tag = [0; HEREDOC_TAG_CAP];
    state.heredoc_tag_len = 0;
    state.mode = ShellMode::Normal;
}

fn activate_pending_heredoc(state: &mut ShellState) {
    if state.pending_heredoc {
        state.pending_heredoc = false;
        state.mode = ShellMode::Heredoc;
    }
}

fn after_finished_token(state: &mut ShellState) {
    if state.restore_command {
        state.at_command = true;
        state.restore_command = false;
    }
}

fn scan_single(bytes: &[u8], i: &mut usize, dollar: bool) -> bool {
    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'\'' {
            *i += 1;
            return true;
        }
        if dollar && c == b'\\' {
            *i += 1;
            if *i < bytes.len() && !is_newline(bytes[*i]) {
                *i += 1;
            }
            continue;
        }
        *i += 1;
    }
    false
}

fn peek_eq(bytes: &[u8], i: usize, s: &[u8]) -> bool {
    bytes.get(i..).map(|r| r.starts_with(s)).unwrap_or(false)
}

fn lex_double_body(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ShellState,
    start: u32,
) -> Result<ShellToken, LexError> {
    let body_start = *i;
    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'"' {
            if *i > body_start {
                return Ok(tok(start, *i, ShellKind::String));
            }
            *i += 1;
            state.mode = ShellMode::Normal;
            after_finished_token(state);
            return Ok(tok(start, *i, ShellKind::String));
        }
        if c == b'\\' {
            *i += 1;
            if *i < bytes.len() && !is_newline(bytes[*i]) {
                *i += 1;
            }
            continue;
        }
        if c == b'$' {
            if *i > body_start {
                return Ok(tok(start, *i, ShellKind::String));
            }
            if peek_eq(bytes, *i, b"$((") {
                if push_nest(state, NEST_DBLARITH, 2) {
                    *i += 3;
                    state.mode = ShellMode::Normal;
                    state.at_command = true;
                    return Ok(tok(start, *i, ShellKind::Punctuator));
                }
                *i += 1;
                continue;
            }
            if peek_eq(bytes, *i, b"$(") {
                if push_nest(state, NEST_DBL, 1) {
                    *i += 2;
                    state.mode = ShellMode::Normal;
                    state.at_command = true;
                    return Ok(tok(start, *i, ShellKind::Punctuator));
                }
                // Depth cap: bytes stay String.
                *i += 1;
                continue;
            }
            *i += 1;
            scan_dollar_ident(bytes, i);
            if *i == body_start + 1 {
                return Ok(tok(start, *i, ShellKind::String));
            }
            return Ok(tok(start, *i, ShellKind::Identifier));
        }
        if is_newline(c) {
            if *i > body_start {
                return Ok(tok(start, *i, ShellKind::String));
            }
            consume_newline(bytes, i);
            return Ok(tok(start, *i, ShellKind::String));
        }
        *i += 1;
    }
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, ShellKind::String))
}

fn operator_at(bytes: &[u8], i: usize) -> Option<(usize, ShellKind, &'static str)> {
    let rest = &bytes[i..];
    if rest.starts_with(b";;&") {
        return Some((3, ShellKind::Punctuator, ";;&"));
    }
    if rest.starts_with(b"<<<") {
        return Some((3, ShellKind::Punctuator, "<<<"));
    }
    if rest.starts_with(b"<<-") {
        return Some((3, ShellKind::Punctuator, "<<-"));
    }
    if rest.starts_with(b";&") {
        return Some((2, ShellKind::Punctuator, ";&"));
    }
    if rest.starts_with(b";;") {
        return Some((2, ShellKind::Punctuator, ";;"));
    }
    if rest.starts_with(b"||") {
        return Some((2, ShellKind::Punctuator, "||"));
    }
    if rest.starts_with(b"&&") {
        return Some((2, ShellKind::Punctuator, "&&"));
    }
    if rest.starts_with(b"<<") {
        return Some((2, ShellKind::Punctuator, "<<"));
    }
    if rest.starts_with(b">>") {
        return Some((2, ShellKind::Punctuator, ">>"));
    }
    if rest.starts_with(b"<&") {
        return Some((2, ShellKind::Punctuator, "<&"));
    }
    if rest.starts_with(b">&") {
        return Some((2, ShellKind::Punctuator, ">&"));
    }
    if rest.starts_with(b"&>") {
        return Some((2, ShellKind::Punctuator, "&>"));
    }
    if rest.starts_with(b"+=") {
        return Some((2, ShellKind::Punctuator, "+="));
    }
    if rest.starts_with(b"[[") {
        return Some((2, ShellKind::Delimiter, "[["));
    }
    if rest.starts_with(b"]]") {
        return Some((2, ShellKind::Delimiter, "]]"));
    }
    match rest.first().copied() {
        Some(b'|') => Some((1, ShellKind::Punctuator, "|")),
        Some(b'&') => Some((1, ShellKind::Punctuator, "&")),
        Some(b';') => Some((1, ShellKind::Punctuator, ";")),
        Some(b'!') => Some((1, ShellKind::Punctuator, "!")),
        Some(b'>') => Some((1, ShellKind::Punctuator, ">")),
        Some(b'<') => Some((1, ShellKind::Punctuator, "<")),
        Some(b'=') => Some((1, ShellKind::Punctuator, "=")),
        Some(b'~') => Some((1, ShellKind::Punctuator, "~")),
        Some(b'*') => Some((1, ShellKind::Punctuator, "*")),
        Some(b'?') => Some((1, ShellKind::Punctuator, "?")),
        Some(b'(') => Some((1, ShellKind::Delimiter, "(")),
        Some(b')') => Some((1, ShellKind::Delimiter, ")")),
        Some(b'{') => Some((1, ShellKind::Delimiter, "{")),
        Some(b'}') => Some((1, ShellKind::Delimiter, "}")),
        Some(b'[') => Some((1, ShellKind::Delimiter, "[")),
        Some(b']') => Some((1, ShellKind::Delimiter, "]")),
        _ => None,
    }
}

fn op_starts_command(op: &str) -> bool {
    matches!(
        op,
        ";" | ";;" | ";&" | ";;&" | "&&" | "||" | "|" | "&" | "(" | "{" | "!" | "`" | "$(" | "$(("
    )
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut ShellState,
    at_sol: &mut bool,
    prev: &mut Option<u8>,
) -> Result<ShellToken, LexError> {
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    if *at_sol && matches!(state.mode, ShellMode::Normal) {
        if state.line_continued {
            state.at_command = false;
            state.line_continued = false;
        } else if state.nest_len == 0 {
            state.at_command = true;
        }
        activate_pending_heredoc(state);
    }

    if state.mode == ShellMode::Heredoc && *at_sol {
        if let Some(end) = heredoc_line_matches(bytes, *i, state) {
            *i = end;
            clear_heredoc(state);
            *at_sol = *i >= bytes.len() || is_newline(bytes.get(*i).copied().unwrap_or(b'\n'));
            return Ok(tok(start, *i, ShellKind::String));
        }
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        *at_sol = false;
        return Ok(tok(start, *i, ShellKind::String));
    }

    match state.mode {
        ShellMode::Single => {
            let closed = scan_single(bytes, i, false);
            if closed {
                state.mode = ShellMode::Normal;
                after_finished_token(state);
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            *at_sol = false;
            return Ok(tok(start, *i, ShellKind::String));
        }
        ShellMode::DollarSingle => {
            let closed = scan_single(bytes, i, true);
            if closed {
                state.mode = ShellMode::Normal;
                after_finished_token(state);
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            *at_sol = false;
            return Ok(tok(start, *i, ShellKind::String));
        }
        ShellMode::Double => {
            let t = lex_double_body(bytes, i, state, start)?;
            *at_sol = false;
            return Ok(t);
        }
        ShellMode::Heredoc | ShellMode::Normal => {}
    }

    let b = bytes[*i];
    if is_newline(b) {
        consume_newline(bytes, i);
        *at_sol = true;
        *prev = Some(b'\n');
        activate_pending_heredoc(state);
        return Ok(tok(start, *i, ShellKind::Whitespace));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        *prev = Some(b' ');
        return Ok(tok(start, *i, ShellKind::Whitespace));
    }

    if b == b'\\' {
        *i += 1;
        if *i >= bytes.len() || is_newline(bytes[*i]) {
            state.line_continued = true;
            *at_sol = false;
            *prev = Some(b'\\');
            return Ok(tok(start, *i, ShellKind::Punctuator));
        }
        *i += 1;
        *at_sol = false;
        *prev = bytes.get(*i - 1).copied();
        return Ok(tok(start, *i, ShellKind::Unknown));
    }

    if b == b'#' && comment_at_word_start(*prev, *at_sol) {
        *i += 1;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        *at_sol = false;
        return Ok(tok(start, *i, ShellKind::Comment));
    }

    if b == b'$' && bytes.get(*i + 1) == Some(&b'\'') {
        *i += 2;
        let closed = scan_single(bytes, i, true);
        if !closed {
            state.mode = ShellMode::DollarSingle;
        } else {
            after_finished_token(state);
        }
        *at_sol = false;
        state.at_command = false;
        return Ok(tok(start, *i, ShellKind::String));
    }

    if b == b'\'' {
        *i += 1;
        let closed = scan_single(bytes, i, false);
        if !closed {
            state.mode = ShellMode::Single;
        } else {
            after_finished_token(state);
        }
        *at_sol = false;
        state.at_command = false;
        return Ok(tok(start, *i, ShellKind::String));
    }

    if b == b'"' {
        *i += 1;
        state.mode = ShellMode::Double;
        *at_sol = false;
        return lex_double_body(bytes, i, state, start);
    }

    if b == b'`' {
        *i += 1;
        *at_sol = false;
        *prev = Some(b'`');
        if nest_top(state).map(|(k, _)| k) == Some(NEST_TICK) {
            let _ = pop_nest(state);
            state.at_command = false;
            return Ok(tok(start, *i, ShellKind::Punctuator));
        }
        let _ = push_nest(state, NEST_TICK, 0);
        state.at_command = true;
        return Ok(tok(start, *i, ShellKind::Punctuator));
    }

    if b == b'$' {
        if peek_eq(bytes, *i, b"$((") {
            *i += 3;
            let _ = push_nest(state, NEST_ARITH, 2);
            state.at_command = true;
            *at_sol = false;
            return Ok(tok(start, *i, ShellKind::Punctuator));
        }
        if peek_eq(bytes, *i, b"$(") {
            *i += 2;
            let _ = push_nest(state, NEST_CMD, 1);
            state.at_command = true;
            *at_sol = false;
            return Ok(tok(start, *i, ShellKind::Punctuator));
        }
        *i += 1;
        scan_dollar_ident(bytes, i);
        if (*i as u32) <= start {
            *i = start as usize + 1;
        }
        *at_sol = false;
        state.at_command = false;
        after_finished_token(state);
        return Ok(tok(start, *i, ShellKind::Identifier));
    }

    if let Some((n, kind, op)) = operator_at(bytes, *i) {
        if op == "<<-" || op == "<<" {
            *i += n;
            parse_heredoc_tag(bytes, i, state, op == "<<-");
            *at_sol = false;
            *prev = Some(b'<');
            state.at_command = false;
            return Ok(tok(start, start as usize + n, ShellKind::Punctuator));
        }
        if op == ")" {
            if let Some((nk, parens)) = nest_top(state) {
                let arith = nk == NEST_ARITH || nk == NEST_DBLARITH;
                if arith && peek_eq(bytes, *i, b"))") && parens <= 2 {
                    *i += 2;
                    let kind_pop = pop_nest(state);
                    if matches!(kind_pop, Some(NEST_DBL | NEST_DBLARITH)) {
                        state.mode = ShellMode::Double;
                    }
                    state.at_command = false;
                    *at_sol = false;
                    *prev = Some(b')');
                    after_finished_token(state);
                    return Ok(tok(start, *i, ShellKind::Punctuator));
                }
                if matches!(nk, NEST_CMD | NEST_DBL | NEST_ARITH | NEST_DBLARITH) {
                    if parens > 1 {
                        nest_paren_add(state, -1);
                        *i += 1;
                        *at_sol = false;
                        *prev = Some(b')');
                        return Ok(tok(start, *i, ShellKind::Delimiter));
                    }
                    *i += 1;
                    let kind_pop = pop_nest(state);
                    if matches!(kind_pop, Some(NEST_DBL | NEST_DBLARITH)) {
                        state.mode = ShellMode::Double;
                    }
                    state.at_command = false;
                    *at_sol = false;
                    *prev = Some(b')');
                    after_finished_token(state);
                    return Ok(tok(start, *i, ShellKind::Punctuator));
                }
            }
        }
        if op == "(" {
            if state.nest_len > 0 {
                nest_paren_add(state, 1);
            }
        }
        *i += n;
        *at_sol = false;
        *prev = bytes.get(*i - 1).copied();
        if op_starts_command(op) {
            state.at_command = true;
        } else {
            state.at_command = false;
        }
        return Ok(tok(start, *i, kind));
    }

    // Assignment prefix at command position: name= / name+=
    if state.at_command && is_ident_start(b) {
        let mut j = *i + 1;
        while j < bytes.len() && is_ident_continue(bytes[j]) {
            j += 1;
        }
        let assign = bytes.get(j) == Some(&b'=')
            || (bytes.get(j) == Some(&b'+') && bytes.get(j + 1) == Some(&b'='));
        if assign {
            *i = j;
            *at_sol = false;
            *prev = bytes.get(*i - 1).copied();
            state.restore_command = true;
            state.at_command = false;
            return Ok(tok(start, *i, ShellKind::Identifier));
        }
        // name()
        let mut k = j;
        while k < bytes.len() && is_space(bytes[k]) {
            k += 1;
        }
        if bytes.get(k) == Some(&b'(') && bytes.get(k + 1) == Some(&b')') {
            *i = j;
            *at_sol = false;
            state.at_command = false;
            return Ok(tok(start, *i, ShellKind::Function));
        }
    }

    // Unquoted word.
    let word_start = *i;
    while *i < bytes.len() && !word_end_byte(bytes[*i]) {
        *i += 1;
    }
    if *i == word_start {
        *i += 1;
        *at_sol = false;
        *prev = Some(b);
        return Ok(tok(start, *i, ShellKind::Unknown));
    }
    let word = &bytes[word_start..*i];
    let kind = classify_plain_word(word, state.at_command);
    let text = std::str::from_utf8(word).unwrap_or("");
    if state.at_command {
        if keeps_command_position(text) {
            state.at_command = true;
        } else if classify_command_word(text).is_some() {
            state.at_command = false;
        } else {
            state.at_command = false;
        }
    }
    after_finished_token(state);
    *at_sol = false;
    *prev = bytes.get(*i - 1).copied();
    Ok(tok(start, *i, kind))
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

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    dialect: ShellDialect,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<ShellToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = ShellState::for_dialect(dialect);
    let mut i = 0usize;
    let mut at_sol = true;
    let mut prev = None;
    let token_cap = bytes.len().saturating_add(8);
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if tokens.len() > token_cap {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
        let start_i = i;
        match lex_one(bytes, &mut i, &mut state, &mut at_sol, &mut prev) {
            Ok(t) => {
                if t.end <= t.start {
                    return Err(LexError::Nonprogress { at: t.start });
                }
                tokens.push(t);
            }
            Err(e) => return Err(e),
        }
        if i == start_i {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
    }
    Ok(tokens)
}

/// Tokenize one display line (without the newline) given incoming continuation.
pub fn lex_line(line: &str, incoming: ShellState) -> (ShellState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut at_sol = true;
    let mut prev = None;
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut at_sol, &mut prev) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    push_role(end, t.kind.role(), &mut out);
                }
                i = end.max(i);
            }
            Err(LexError::Nonprogress { .. }) => {
                if i == start && i < bytes.len() {
                    i += 1;
                    push_role(i, TokenRole::Unknown, &mut out);
                } else {
                    break;
                }
            }
            Err(_) => break,
        }
        if i == start {
            if i < bytes.len() {
                i += 1;
                push_role(i, TokenRole::Unknown, &mut out);
            } else {
                break;
            }
        }
    }
    if i < bytes.len() {
        let role = match state.mode {
            ShellMode::Single | ShellMode::DollarSingle | ShellMode::Double | ShellMode::Heredoc => {
                TokenRole::String
            }
            ShellMode::Normal => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

/// Convert document tokens into public spans.
pub fn document_spans(_bytes: &[u8], tokens: &[ShellToken]) -> Vec<TokenSpan> {
    tokens
        .iter()
        .map(|t| TokenSpan::new(t.start, t.end, t.kind.role()))
        .collect()
}
