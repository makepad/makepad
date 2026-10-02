// Makepad script streaming tokenizer

use crate::colorhex::hex_bytes_to_u32;
use crate::heap::*;
use crate::makepad_live_id::LiveId;
use crate::makepad_live_id_macros::*;
use crate::value::*;

#[derive(Copy, Clone, Debug)]
pub enum ScriptToken {
    End,
    StreamEnd,
    Identifier(LiveId),
    Operator(LiveId),
    Separator(LiveId),
    OpenCurly,
    CloseCurly,
    OpenRound,
    CloseRound,
    OpenSquare,
    CloseSquare,
    StringUnfinished,
    String(ScriptValue),
    F32(f32),
    U32(u32),
    I32(i32),
    F16(f32),
    F64(f64),
    U40(u64),
    Color(u32),
    RustValue(u32),
}

impl ScriptToken {
    pub fn identifier(&self) -> LiveId {
        match self {
            ScriptToken::Identifier(id) => *id,
            _ => id!(),
        }
    }
    pub fn operator(&self) -> LiveId {
        match self {
            ScriptToken::Operator(id) => *id,
            _ => id!(),
        }
    }
    pub fn separator(&self) -> LiveId {
        match self {
            ScriptToken::Separator(id) => *id,
            _ => id!(),
        }
    }
    pub fn f64(&self) -> f64 {
        match self {
            ScriptToken::F64(v) => *v,
            _ => 0.0,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ScriptToken::F64(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_u40(&self) -> Option<u64> {
        match self {
            ScriptToken::U40(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            ScriptToken::F32(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            ScriptToken::U32(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            ScriptToken::I32(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_f16(&self) -> Option<f32> {
        match self {
            ScriptToken::F16(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_color(&self) -> Option<u32> {
        match self {
            ScriptToken::Color(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_string(&self) -> Option<ScriptValue> {
        match self {
            ScriptToken::String(v) => Some(*v),
            ScriptToken::StringUnfinished => Some(ScriptValue::EMPTY_STRING),
            _ => None,
        }
    }
    pub fn as_rust_value(&self) -> Option<u32> {
        match self {
            ScriptToken::RustValue(v) => Some(*v),
            _ => None,
        }
    }

    pub fn is_identifier(&self) -> bool {
        match self {
            ScriptToken::Identifier { .. } => true,
            _ => false,
        }
    }
    pub fn is_operator(&self) -> bool {
        match self {
            ScriptToken::Operator(_) => true,
            _ => false,
        }
    }
    pub fn is_open_curly(&self) -> bool {
        match self {
            ScriptToken::OpenCurly => true,
            _ => false,
        }
    }
    pub fn is_close_curly(&self) -> bool {
        match self {
            ScriptToken::CloseCurly => true,
            _ => false,
        }
    }
    pub fn is_open_round(&self) -> bool {
        match self {
            ScriptToken::OpenRound => true,
            _ => false,
        }
    }
    pub fn is_close_round(&self) -> bool {
        match self {
            ScriptToken::CloseRound => true,
            _ => false,
        }
    }
    pub fn is_open_square(&self) -> bool {
        match self {
            ScriptToken::OpenSquare => true,
            _ => false,
        }
    }
    pub fn is_close_square(&self) -> bool {
        match self {
            ScriptToken::CloseSquare => true,
            _ => false,
        }
    }
    pub fn is_string(&self) -> bool {
        match self {
            ScriptToken::StringUnfinished | ScriptToken::String(_) => true,
            _ => false,
        }
    }
    pub fn is_f64(&self) -> bool {
        match self {
            ScriptToken::F64(_) => true,
            _ => false,
        }
    }
    pub fn is_u40(&self) -> bool {
        match self {
            ScriptToken::U40(_) => true,
            _ => false,
        }
    }
    pub fn is_f32(&self) -> bool {
        match self {
            ScriptToken::F32(_) => true,
            _ => false,
        }
    }
    pub fn is_u32(&self) -> bool {
        match self {
            ScriptToken::U32(_) => true,
            _ => false,
        }
    }
    pub fn is_i32(&self) -> bool {
        match self {
            ScriptToken::I32(_) => true,
            _ => false,
        }
    }
    pub fn is_f16(&self) -> bool {
        match self {
            ScriptToken::F16(_) => true,
            _ => false,
        }
    }
    pub fn is_color(&self) -> bool {
        match self {
            ScriptToken::Color(_) => true,
            _ => false,
        }
    }
    pub fn is_rust_value(&self) -> bool {
        match self {
            ScriptToken::RustValue(_) => true,
            _ => false,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ScriptTokenPos {
    pub token: ScriptToken,
    pos: usize,
    /// True if whitespace containing a newline separated this token from the
    /// previous one. Lets the parser keep statements newline-delimited (a
    /// continuation token on a new line begins a new statement, Go/Swift-style).
    pub preceded_by_newline: bool,
    /// True if any whitespace separated this token from the previous one.
    /// Inside an array literal `[a] [b]` is two elements, `a[b]` an index.
    pub preceded_by_space: bool,
    /// The characters the token was written in, as char indices into the
    /// source (`start` first character, `end` one past the last). Exact for
    /// every token but an unterminated string.
    start: u32,
    end: u32,
    /// The unit a number literal was written in (`0.12s`, `120ms`, `90deg`): its
    /// value is already in seconds or radians; the unit says what it is.
    pub unit: Option<ScriptUnit>,
}

/// The unit suffix of a number literal: what a value means (an editor picks its
/// control by it). The token's value is in the base unit: seconds for time,
/// radians for angles, virtual pixels for lengths.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScriptUnit {
    /// `0.12s`: seconds.
    Seconds,
    /// `120ms`: milliseconds, the value in seconds.
    Millis,
    /// `90deg`: degrees, the value in radians.
    Degrees,
    /// `1.5rad`: radians (the base unit of angles).
    Radians,
    /// `12px`: virtual (DPI-scaled) pixels, the base unit of lengths and positions.
    Pixels,
}

impl ScriptUnit {
    pub fn parse(suffix: &str) -> Option<Self> {
        match suffix {
            "s" => Some(Self::Seconds),
            "ms" => Some(Self::Millis),
            "deg" => Some(Self::Degrees),
            "rad" => Some(Self::Radians),
            "px" => Some(Self::Pixels),
            _ => None,
        }
    }
    /// The value of `v` written in this unit, in the base unit.
    pub fn to_base(self, v: f64) -> f64 {
        match self {
            Self::Seconds | Self::Radians | Self::Pixels => v,
            Self::Millis => v / 1000.0,
            Self::Degrees => v * std::f64::consts::PI / 180.0,
        }
    }
}

impl ScriptTokenPos {
    /// The token's characters in the source, as char indices.
    pub fn span(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }

    /// Where the token starts in its body's code (a character index).
    pub fn pos(&self) -> usize {
        self.pos
    }
}

/// One captured `/** ... */` doc annotation (see `ScriptTokenizer::docs`).
#[derive(Clone, Debug)]
pub struct ScriptTokDoc {
    /// Index the NEXT token gets (`tokens.len()` at capture end).
    pub next_token: u32,
    pub text: String,
}

#[derive(Clone, Default, Eq, PartialEq)]
enum State {
    #[default]
    Whitespace,
    Identifier,
    Operator,
    RustValue,
    String(bool),
    EscapeInString(bool),
    /// `\` then a newline in a string: the newline and the next line's
    /// leading whitespace are skipped (Rust's line continuation).
    ContinueInString(bool),
    /// `r` then this many `#`: a raw string opens at the `"`.
    RawStringOpen(usize),
    /// Inside `r#…"…"#…` with this many `#`: no escapes, any line.
    RawString(usize),
    /// A `"` inside a raw string with this many `#`, then `seen` of them:
    /// the string ends when they match.
    RawStringClose(usize, usize),
    UnicodeHexInString(bool),
    UnicodeCurlyInString(bool),
    AsciiHexInString(bool),
    BlockComment(usize),
    MaybeEndBlock(usize),
    /// Just entered `/*`; a following `*` may open a `/**name*/` doc.
    BlockCommentStart,
    /// Saw `/**`; the next char decides doc (`/**x`), empty (`/**/`) or
    /// plain (`/***`, Rust convention).
    BlockDocStart,
    /// Inside `/**...*/`: text accumulates into `temp`.
    BlockDoc,
    /// Saw `*` inside a block doc; `/` closes it.
    BlockDocMaybeEnd,
    LineComment,
    /// Just after `//`: a third `/` makes a `///` doc line (a fourth, a plain comment).
    LineCommentStart,
    /// Just after `///`: a fourth `/` makes it plain (`////`), else the doc text begins.
    LineDocStart,
    /// Inside a `///` doc line: text accumulates into `temp`.
    LineDoc,
    Number,
    Color,
}

impl State {
    /// A token is being lexed (not whitespace or a comment).
    fn lexing_token(&self) -> bool {
        matches!(
            self,
            State::Identifier
                | State::Operator
                | State::RustValue
                | State::Number
                | State::Color
                | State::String(_)
                | State::EscapeInString(_)
                | State::RawStringOpen(_)
                | State::ContinueInString(_)
                | State::RawString(_)
                | State::RawStringClose(..)
                | State::UnicodeHexInString(_)
                | State::UnicodeCurlyInString(_)
                | State::AsciiHexInString(_)
        )
    }

    fn in_string(&self) -> bool {
        matches!(
            self,
            State::String(_)
                | State::EscapeInString(_)
                | State::ContinueInString(_)
                | State::RawString(_)
                | State::RawStringClose(..)
                | State::UnicodeHexInString(_)
                | State::UnicodeCurlyInString(_)
                | State::AsciiHexInString(_)
        )
    }
}

#[derive(Default)]
pub struct ScriptTokenizer {
    pos: usize,
    /// Set when a newline is consumed; stamped onto (and cleared by) the next
    /// emitted token as `preceded_by_newline`.
    newline_pending: bool,
    /// Same for any whitespace (`preceded_by_space`).
    space_pending: bool,
    /// The last token is the provisional one `push_pending_token` added.
    provisional: bool,
    pub tokens: Vec<ScriptTokenPos>,
    /// Captured `/** ... */` doc annotations, keyed by the index the NEXT
    /// token gets (`tokens.len()` at capture end). ONE form; position
    /// determines meaning at resolution time (`docs::resolve_docs`):
    /// before `key:` it documents the field, before an object literal it
    /// documents the object, immediately before a value literal it names
    /// that value (`/**glow tint*/ #8f0`). The parser never sees these;
    /// `//` and `/* */` remain plain discarded comments.
    pub docs: Vec<ScriptTokDoc>,
    pub original: String,
    /// The char index each line of `original` after the first starts at
    /// (one past each `\n`): token positions to rows and columns by a
    /// binary search, not a scan of the source.
    line_starts: Vec<u32>,
    unfinished: String,
    temp: String,
    /// Where a number's unit suffix starts in `temp` (`0.12s`), when it has one.
    unit_start: Option<usize>,
    state: State,
    /// First character of the token being lexed, and one past its last
    /// when it is emitted (see `ScriptTokenPos::span`).
    lex_start: usize,
    tok_end: usize,
}

pub struct ScriptLoc {
    pub row: usize,
    pub col: usize,
}

impl ScriptTokenizer {
    pub fn clear(&mut self) {
        self.pos = 0;
        self.newline_pending = false;
        self.space_pending = false;
        self.provisional = false;
        self.tokens.clear();
        self.docs.clear();
        self.original.clear();
        self.line_starts.clear();
        self.unfinished.clear();
        self.temp.clear();
        self.state = State::Whitespace;
        self.lex_start = 0;
        self.tok_end = 0;
    }

    /// Iterate over all string values in the token stream.
    /// Used by GC to mark tokenizer strings as roots.
    pub fn iter_strings(&self) -> impl Iterator<Item = ScriptValue> + '_ {
        self.tokens.iter().filter_map(|tp| {
            if let ScriptToken::String(v) = tp.token {
                Some(v)
            } else {
                None
            }
        })
    }

    /// The zero-based row and column (characters) of the first character
    /// of token `tok_index`: where it is written, from its lexed span (a
    /// token's `pos` may sit a character into it).
    pub fn token_start_row_col(&self, tok_index: u32) -> Option<(u32, u32)> {
        let char_index = self.tokens.get(tok_index as usize)?.start as usize;
        if char_index >= self.pos {
            return None;
        }
        let line = self.line_starts.partition_point(|&start| start as usize <= char_index);
        let line_start = if line == 0 { 0 } else { self.line_starts[line - 1] as usize };
        Some((line as u32, (char_index - line_start) as u32))
    }

    pub fn token_index_to_row_col(&self, tok_index: u32) -> Option<(u32, u32)> {
        let char_index = self.tokens[tok_index as usize].pos;
        if char_index >= self.pos {
            return None;
        }
        let line = self.line_starts.partition_point(|&start| start as usize <= char_index);
        let line_start = if line == 0 { 0 } else { self.line_starts[line - 1] as usize };
        Some((line as u32, (char_index - line_start) as u32))
    }


    /// Emits a token, stamping whether a newline preceded it (and clearing the
    /// pending flag). All token emission funnels through here.
    fn push_tok(&mut self, pos: usize, token: ScriptToken) {
        let preceded_by_newline = self.newline_pending;
        let preceded_by_space = self.space_pending;
        self.newline_pending = false;
        self.space_pending = false;
        self.tokens.push(ScriptTokenPos {
            token,
            pos,
            preceded_by_newline,
            preceded_by_space,
            start: self.lex_start as u32,
            end: self.tok_end as u32,
            unit: None,
        });
    }

    /// Whether token `i` was preceded by whitespace (see `preceded_by_space`).
    pub fn token_preceded_by_space(&self, i: u32) -> bool {
        self.tokens.get(i as usize).is_some_and(|t| t.preceded_by_space)
    }

    /// Whether token `i` was preceded by a newline (see `preceded_by_newline`).
    pub fn token_preceded_by_newline(&self, i: u32) -> bool {
        self.tokens.get(i as usize).is_some_and(|t| t.preceded_by_newline)
    }


    pub fn pos_to_loc(&self, pos: usize) -> Option<ScriptLoc> {
        let mut row = 0;
        let mut col = 0;
        for (i, c) in self.original.chars().enumerate() {
            if c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
            if i >= pos {
                return Some(ScriptLoc { row, col });
            }
        }
        None
    }

    /// Whether lexing stopped inside a string literal.
    fn in_string(&self) -> bool {
        matches!(
            self.state,
            State::String(_)
                | State::EscapeInString(_)
                | State::UnicodeHexInString(_)
                | State::UnicodeCurlyInString(_)
                | State::AsciiHexInString(_)
        )
    }

    /// The source is complete: emit the token still being lexed at its end.
    /// A number, identifier, operator or color is only emitted by the
    /// character after it, so a source ending in one without a trailing
    /// newline lost it (`x * 100` parsed as `x *`). An unterminated string
    /// stays StringUnfinished.
    pub fn finish(&mut self, heap: &mut ScriptHeap) {
        if self.state != State::Whitespace && !self.in_string() {
            self.tokenize("\n", heap);
        }
    }

    /// Streaming: push the token still being lexed at the end of the source
    /// so far, lexed as if the source ended there, as a provisional last
    /// token (more source may still extend it: `12`, then `3`). The next
    /// `tokenize` drops it and lexes on; it stays until then so error
    /// locations can still resolve it. Returns whether one was pushed. See
    /// `finish`.
    pub fn push_pending_token(&mut self, heap: &mut ScriptHeap) -> bool {
        if self.provisional {
            // no new source since the last push
            return true;
        }
        if self.state == State::Whitespace || self.in_string() {
            return false;
        }
        let pos = self.pos;
        let newline_pending = self.newline_pending;
        let space_pending = self.space_pending;
        let temp = self.temp.clone();
        let state = self.state.clone();
        let (lex_start, tok_end) = (self.lex_start, self.tok_end);
        let (tokens_len, docs_len, original_len, lines_len) =
            (self.tokens.len(), self.docs.len(), self.original.len(), self.line_starts.len());
        self.tokenize("\n", heap);
        let token = self.tokens.get(tokens_len).copied();
        self.pos = pos;
        self.newline_pending = newline_pending;
        self.space_pending = space_pending;
        self.temp = temp;
        self.state = state;
        self.lex_start = lex_start;
        self.tok_end = tok_end;
        self.tokens.truncate(tokens_len);
        self.docs.truncate(docs_len);
        self.original.truncate(original_len);
        self.line_starts.truncate(lines_len);
        if let Some(token) = token {
            self.tokens.push(token);
            self.provisional = true;
        }
        self.provisional
    }

    fn emit_rust_value(&mut self) {
        let number = if let Ok(v) = self.temp.parse::<u32>() {
            self.temp.clear();
            v
        } else {
            0
        };
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::RustValue(number));
    }

    fn emit_f64(&mut self) {
        if let Some(k) = self.unit_start.take() {
            let suffix = self.temp.split_off(k);
            match ScriptUnit::parse(&suffix) {
                Some(unit) => {
                    let len = self.temp.chars().count() + suffix.chars().count();
                    let v = self.temp.parse::<f64>().unwrap_or(0.0);
                    self.temp.clear();
                    self.push_tok(self.pos - len, ScriptToken::F64(unit.to_base(v)));
                    if let Some(t) = self.tokens.last_mut() {
                        t.unit = Some(unit);
                    }
                }
                None => {
                    // not a unit: the number, then an identifier
                    self.emit_f64();
                    self.temp = suffix;
                    self.emit_identifier();
                }
            }
            return;
        }
        // Measure before clearing: a float token's position was taken from
        // an already-emptied `temp`, so it pointed one past its terminator —
        // a float ending a line resolved to the NEXT line, column 0.
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        let number = if let Ok(v) = self.temp.parse::<f64>() {
            // allow the shader compiler to recognise the difference btween 1 and 1.
            if !(self.temp.contains('.') || self.temp.contains('e') || self.temp.contains('E'))
                && v <= 0xFF_FFFF_FFFFu64 as f64
            {
                self.temp.clear();
                self.push_tok(self.pos - len, ScriptToken::U40(v as u64));
                return;
            }
            self.temp.clear();
            v
        } else {
            0.0
        };
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::F64(number));
    }

    fn emit_f32(&mut self) {
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        let number = if let Ok(v) = self.temp.parse::<f32>() {
            self.temp.clear();
            v
        } else {
            0.0
        };
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::F32(number));
    }

    fn emit_u32(&mut self) {
        let number = if let Ok(v) = self.temp.parse::<u32>() {
            self.temp.clear();
            v
        } else {
            0
        };
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::U32(number));
    }

    fn emit_i32(&mut self) {
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        let number = if let Ok(v) = self.temp.parse::<i32>() {
            self.temp.clear();
            v
        } else {
            0
        };
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::I32(number));
    }

    fn emit_f16(&mut self) {
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        let number = if let Ok(v) = self.temp.parse::<f32>() {
            self.temp.clear();
            v
        } else {
            0.0
        };
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::F16(number));
    }

    fn emit_identifier(&mut self) {
        let id = match LiveId::from_str_with_lut(&self.temp) {
            Err(str) => {
                println!(
                    "--WARNING-- LiveId LUT collision between {} and {}",
                    self.temp, str
                );
                LiveId::from_str(&self.temp)
            }
            Ok(id) => id,
        };
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::Identifier(id));
    }

    fn emit_operator(&mut self) {
        if self.temp.len() == 0 {
            return;
        }
        let id = match LiveId::from_str_with_lut(&self.temp) {
            Err(str) => {
                println!(
                    "--WARNING-- LiveId LUT collision between {} and {}",
                    self.temp, str
                );
                LiveId::from_str(&self.temp)
            }
            Ok(id) => id,
        };
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::Operator(id));
    }

    fn emit_separator(&mut self, c: char) {
        if self.temp.len() != 0 {
            panic!()
        }
        self.lex_start = self.pos.saturating_sub(1);
        self.tok_end = self.pos;
        self.temp.push(c);
        let id = match LiveId::from_str_with_lut(&self.temp) {
            Err(str) => {
                println!(
                    "--WARNING-- LiveId LUT collision between {} and {}",
                    self.temp, str
                );
                LiveId::from_str(&self.temp)
            }
            Ok(id) => id,
        };
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::Separator(id));
    }

    fn emit_color(&mut self) {
        let color = match hex_bytes_to_u32(&self.temp.as_bytes()) {
            Err(()) => 0xff00ffff,
            Ok(color) => color,
        };
        // `pos` counts chars (token_index_to_row_col iterates chars), so the
        // token length must too or a multibyte identifier underflows.
        let len = self.temp.chars().count();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::Color(color));
    }

    /// A single-character token for the character just consumed. The loop
    /// has already advanced `pos` past it, so its position is one back;
    /// every other token records its first character the same way.
    fn emit_token_here(&mut self, token: ScriptToken) {
        self.lex_start = self.pos.saturating_sub(1);
        self.tok_end = self.pos;
        self.push_tok(self.pos.saturating_sub(1), token)
    }

    fn append_unfinished_string(&mut self, c: char) {
        if let Some(ScriptTokenPos {
            token: ScriptToken::StringUnfinished,
            ..
        }) = self.tokens.last_mut()
        {
            self.unfinished.push(c);
        } else {
            self.unfinished.clear();
            self.unfinished.push(c);
            self.push_tok(self.pos, ScriptToken::StringUnfinished);
        }
    }

    /// If the last token is `StringUnfinished`, intern the unfinished buffer content
    /// via the heap and return it as a ScriptValue. Does NOT modify the token — the
    /// tokenizer state remains unchanged for the next `tokenize()` call.
    /// Used at incremental parsing boundaries so the parser gets the real partial string.
    pub fn intern_unfinished_string(&mut self, heap: &mut ScriptHeap) -> Option<ScriptValue> {
        if let Some(ScriptTokenPos {
            token: ScriptToken::StringUnfinished,
            ..
        }) = self.tokens.last()
        {
            Some(heap.new_string_from_str(&self.unfinished))
        } else {
            None
        }
    }

    fn finish_string(&mut self, heap: &mut ScriptHeap) {
        self.tok_end = self.pos;
        if let Some(ScriptTokenPos {
            token: ScriptToken::StringUnfinished,
            ..
        }) = self.tokens.last()
        {
            if let Some(ScriptTokenPos {
                token: ScriptToken::StringUnfinished,
                pos,
                ..
            }) = self.tokens.pop()
            {
                let v = heap.new_string_from_str(&self.unfinished);
                self.unfinished.clear();
                self.push_tok(pos, ScriptToken::String(v))
            }
        } else {
            self.push_tok(self.pos, ScriptToken::String(ScriptValue::EMPTY_STRING))
        }
    }

    pub fn tokenize(&mut self, new_chars: &str, heap: &mut ScriptHeap) -> &[ScriptTokenPos] {
        if self.provisional {
            self.provisional = false;
            self.tokens.pop();
        }
        let mut iter = new_chars.chars();

        fn is_operator(c: char) -> bool {
            c == '!'
                || c == '^'
                || c == '&'
                || c == '*'
                || c == '+'
                || c == '-'
                || c == '|'
                || c == '?'
                || c == ':'
                || c == '='
                || c == '@'
                || c == '>'
                || c == '<'
                || c == '.'
                || c == '/'
                || c == '~'
                || c == '%'
        }
        fn is_separator(c: char) -> bool {
            c == ',' || c == ';'
        }
        fn is_block(c: char) -> Option<ScriptToken> {
            match c {
                '{' => Some(ScriptToken::OpenCurly),
                '}' => Some(ScriptToken::CloseCurly),
                '[' => Some(ScriptToken::OpenSquare),
                ']' => Some(ScriptToken::CloseSquare),
                '(' => Some(ScriptToken::OpenRound),
                ')' => Some(ScriptToken::CloseRound),
                _ => None,
            }
        }
        // unfinished string at the end
        let start = if let Some(ScriptTokenPos {
            token: ScriptToken::StringUnfinished,
            ..
        }) = self.tokens.last_mut()
        {
            self.tokens.len() - 1
        } else {
            self.tokens.len()
        };

        while let Some(c) = iter.next() {
            self.original.push(c);
            self.pos += 1;
            self.tok_end = self.pos - 1;
            let was_lexing = self.state.lexing_token() && !(self.state == State::Operator && self.temp.is_empty());
            let was_in_string = self.state.in_string();
            let tokens_before = self.tokens.len();
            if c == '\n' {
                self.line_starts.push(self.pos as u32);
            }
            match self.state {
                State::Whitespace => {
                    if c.is_numeric() {
                        self.state = State::Number;
                        self.temp.push(c);
                    } else if c == '_' || c == '$' || c.is_alphabetic() {
                        self.state = State::Identifier;
                        self.temp.push(c);
                    } else if c == '#' {
                        self.state = State::Color;
                    } else if is_separator(c) {
                        self.emit_separator(c);
                    } else if is_operator(c) {
                        self.state = State::Operator;
                        self.temp.push(c);
                    } else if c == '"' {
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.state = State::String(false);
                    } else if let Some(tok) = is_block(c) {
                        self.emit_token_here(tok);
                    }
                }
                State::Identifier => {
                    // `r"…"` / `r#"…"#`: a raw string, as in Rust
                    if self.temp == "r" && (c == '"' || c == '#') {
                        if c == '"' {
                            self.temp.clear();
                            self.state = State::RawString(0);
                        } else {
                            self.state = State::RawStringOpen(1);
                        }
                        continue;
                    }
                    if c == '_' || c == '$' || c.is_alphanumeric() {
                        self.temp.push(c);
                    } else if c.is_whitespace() {
                        self.emit_identifier();
                        self.state = State::Whitespace;
                    } else if is_operator(c) {
                        self.emit_identifier();
                        self.state = State::Operator;
                        self.temp.push(c);
                    } else if is_separator(c) {
                        self.emit_identifier();
                        self.emit_separator(c);
                        self.state = State::Whitespace;
                    } else if c == '#' {
                        self.emit_identifier();
                        self.state = State::Color;
                    } else if let Some(tok) = is_block(c) {
                        self.emit_identifier();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    } else if c == '"' {
                        self.emit_identifier();
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.emit_identifier();
                        self.state = State::String(false);
                    } else {
                        self.emit_identifier();
                        self.state = State::Whitespace;
                    }
                }
                State::Operator => {
                    // Helper to check if a string is a valid operator or valid prefix of an operator
                    fn is_valid_operator(s: &str) -> bool {
                        matches!(
                            s,
                            // Single character operators
                            "!" | "~" | "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" |
                            "<" | ">" | "=" | "." | "?" | ":" | "@" |
                            // Double character operators
                            "==" | "!=" | "<=" | ">=" | "&&" | "||" | "|?" | "??" |
                            "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | ":=" |
                            "<<" | ">>" | ".." | "->" | ".?" | ">:" | "<:" | "^:" | "+:" | "?=" |
                            "++" | "-:" | "=>" |
                            "/*" | "//" |
                            // Triple character operators
                            "===" | "!==" | "<<=" | ">>=" | "..."
                        )
                    }

                    fn could_be_operator_prefix(s: &str) -> bool {
                        // Check if s could be the start of a valid multi-char operator
                        matches!(
                            s,
                            // Single chars that could become 2-char operators
                            "!" |  // !=, !==
                            "=" |  // ==, ===
                            "<" |  // <=, <<, <:, <<=
                            ">" |  // >=, >>, >:, >>=
                            "&" |  // &&
                            "|" |  // ||, |?, |=
                            "+" |  // +=, +:, ++
                            "-" |  // -=, ->, -:
                            "*" |  // *=
                            "/" |  // /=, /*, //
                            "%" |  // %=
                            "^" |  // ^=, ^:
                            ":" |  // :=
                            "." |  // .., .?, ...
                            "?" |  // ?=, ??
                            // Double chars that could become 3-char operators
                            "==" | // ===
                            "!=" | // !==
                            "<<" | // <<=
                            ">>" | // >>=
                            ".." // ...
                        )
                    }

                    // Special case: @( starts a RustValue
                    if self.temp == "@" && c == '(' {
                        self.temp.clear();
                        self.state = State::RustValue;
                        continue;
                    }

                    // Handle non-operator characters - emit current operator and transition
                    if c.is_whitespace() {
                        self.emit_operator();
                        self.state = State::Whitespace;
                    } else if c.is_numeric() {
                        // Handle .5 as 0.5 (float literal starting with dot)
                        if self.temp == "." {
                            self.temp.push(c);
                            self.state = State::Number;
                        } else {
                            self.emit_operator();
                            self.state = State::Number;
                            self.temp.push(c);
                        }
                    } else if is_separator(c) {
                        self.emit_operator();
                        self.emit_separator(c);
                        self.state = State::Whitespace;
                    } else if c == '_' || c == '$' || c.is_alphabetic() {
                        self.emit_operator();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    } else if c == '#' {
                        self.emit_operator();
                        self.state = State::Color;
                    } else if c == '"' {
                        self.emit_operator();
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.emit_operator();
                        self.state = State::String(false);
                    } else if let Some(tok) = is_block(c) {
                        self.emit_operator();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    } else if is_operator(c) {
                        // Try to extend the current operator
                        let mut extended = self.temp.clone();
                        extended.push(c);

                        if is_valid_operator(&extended) || could_be_operator_prefix(&extended) {
                            // Valid extension, keep building
                            self.temp.push(c);
                        } else {
                            // Can't extend - emit current operator and start new one
                            self.emit_operator();
                            self.temp.push(c);
                        }
                    } else {
                        self.emit_operator();
                        self.state = State::Whitespace;
                    }

                    // Check for comment start
                    if self.temp == "/*" {
                        self.state = State::BlockCommentStart;
                        self.temp.clear();
                    } else if self.temp == "//" {
                        self.state = State::LineCommentStart;
                        self.temp.clear();
                    }
                    // Emit complete operators that can't be extended
                    else if is_valid_operator(&self.temp) && !could_be_operator_prefix(&self.temp)
                    {
                        self.tok_end = self.pos;
                        self.emit_operator();
                    }
                }
                State::RawStringOpen(hashes) => {
                    if c == '#' {
                        self.state = State::RawStringOpen(hashes + 1);
                    } else if c == '"' {
                        self.temp.clear();
                        self.state = State::RawString(hashes);
                    } else {
                        // `r#x` is no raw string: the identifier `r`
                        self.emit_identifier();
                        self.state = State::Whitespace;
                    }
                }
                State::RawString(hashes) => {
                    if c == '"' {
                        if hashes == 0 {
                            self.finish_string(heap);
                            self.state = State::Whitespace;
                        } else {
                            self.state = State::RawStringClose(hashes, 0);
                        }
                    } else {
                        self.append_unfinished_string(c);
                    }
                }
                State::RawStringClose(hashes, seen) => {
                    if c == '#' && seen + 1 == hashes {
                        self.finish_string(heap);
                        self.state = State::Whitespace;
                    } else if c == '#' {
                        self.state = State::RawStringClose(hashes, seen + 1);
                    } else {
                        // not the closer: the `"` and the `#`s were text
                        self.append_unfinished_string('"');
                        for _ in 0..seen {
                            self.append_unfinished_string('#');
                        }
                        if c == '"' {
                            self.state = State::RawStringClose(hashes, 0);
                        } else {
                            self.append_unfinished_string(c);
                            self.state = State::RawString(hashes);
                        }
                    }
                }
                State::ContinueInString(double) => {
                    if !c.is_whitespace() {
                        self.state = State::String(double);
                        if c == '\\' {
                            self.temp.clear();
                            self.state = State::EscapeInString(double);
                        } else if (double && c == '"') || (!double && c == '\'') {
                            self.finish_string(heap);
                            self.state = State::Whitespace;
                        } else {
                            self.append_unfinished_string(c);
                        }
                    }
                }
                State::EscapeInString(double) => {
                    // ok lets see what we have for an escape character sequence
                    if c == '\\' {
                        self.append_unfinished_string('\\');
                        self.state = State::String(double);
                    } else if c == '"' {
                        self.append_unfinished_string('"');
                        self.state = State::String(double);
                    } else if c == '\'' {
                        self.append_unfinished_string('\'');
                        self.state = State::String(double);
                    } else if c == 'r' {
                        self.append_unfinished_string('\r');
                        self.state = State::String(double);
                    } else if c == 'n' {
                        self.append_unfinished_string('\n');
                        self.state = State::String(double);
                    } else if c == 't' {
                        self.append_unfinished_string('\t');
                        self.state = State::String(double);
                    } else if c == '0' {
                        self.append_unfinished_string('\0');
                        self.state = State::String(double);
                    } else if c == 'x' {
                        self.state = State::AsciiHexInString(double);
                    } else if c == 'u' {
                        self.state = State::UnicodeHexInString(double);
                    } else if c == '\n' {
                        self.state = State::ContinueInString(double);
                    } else {
                        // not an escape: the backslash and the char stay text
                        self.append_unfinished_string('\\');
                        self.append_unfinished_string(c);
                        self.state = State::String(double);
                    }
                }
                State::AsciiHexInString(double) => {
                    self.temp.push(c);
                    if self.temp.len() == 2 {
                        if let Ok(v) = i64::from_str_radix(&self.temp, 16) {
                            self.append_unfinished_string(v as u8 as char);
                        }
                        self.temp.clear();
                        self.state = State::String(double);
                    }
                }
                State::UnicodeHexInString(double) => {
                    if c == '{' {
                        self.state = State::UnicodeCurlyInString(double);
                    } else {
                        // its kinda unknown how long we need to keep pushing sself
                        self.temp.push(c);
                        if self.temp.len() == 4 {
                            if let Ok(v) = i64::from_str_radix(&self.temp, 16) {
                                if let Some(v) = char::from_u32(v as u32) {
                                    self.append_unfinished_string(v);
                                }
                            }
                            self.temp.clear();
                            self.state = State::String(double);
                        }
                    }
                }
                State::UnicodeCurlyInString(double) => {
                    if c == '}' {
                        if let Ok(v) = i64::from_str_radix(&self.temp, 16) {
                            if let Some(v) = char::from_u32(v as u32) {
                                self.append_unfinished_string(v);
                            }
                        }
                        self.temp.clear();
                        self.state = State::String(double);
                    } else {
                        self.temp.push(c);
                    }
                }
                State::String(double) => {
                    // check last token is
                    if c == '\\' {
                        // escape char
                        self.temp.clear();
                        self.state = State::EscapeInString(double);
                    } else if (double && c == '"') || (!double && c == '\'') {
                        self.finish_string(heap);
                        self.state = State::Whitespace;
                    } else {
                        self.append_unfinished_string(c);
                    }
                }
                State::BlockComment(depth) => {
                    if c == '*' {
                        // end block comment
                        self.state = State::MaybeEndBlock(depth);
                    }
                }
                State::MaybeEndBlock(depth) => {
                    if c == '/' {
                        // end block comment
                        if depth > 0 {
                            self.state = State::BlockComment(depth - 1)
                        } else {
                            self.state = State::Whitespace;
                        }
                    } else {
                        self.state = State::BlockComment(depth)
                    }
                }
                State::LineComment => {
                    if c == '\n' {
                        // end line comment
                        self.state = State::Whitespace;
                    }
                }
                State::LineCommentStart => {
                    self.state = if c == '/' {
                        State::LineDocStart
                    } else if c == '\n' {
                        State::Whitespace
                    } else {
                        State::LineComment
                    };
                }
                State::LineDocStart => {
                    if c == '/' {
                        self.state = State::LineComment;
                    } else if c == '\n' {
                        self.state = State::Whitespace;
                    } else {
                        self.temp.clear();
                        self.temp.push(c);
                        self.state = State::LineDoc;
                    }
                }
                State::LineDoc => {
                    if c == '\n' {
                        // `///text`: a doc for what follows, as `/**text*/` (Rust's doc line)
                        let text = self.temp.trim().to_string();
                        if !text.is_empty() {
                            self.docs.push(ScriptTokDoc { next_token: self.tokens.len() as u32, text });
                        }
                        self.temp.clear();
                        self.state = State::Whitespace;
                    } else {
                        self.temp.push(c);
                    }
                }
                State::BlockCommentStart => {
                    if c == '*' {
                        self.state = State::BlockDocStart;
                    } else if c == '/' {
                        // `/*/` : half-open plain comment, still open
                        self.state = State::BlockComment(0);
                    } else {
                        self.state = State::BlockComment(0);
                    }
                }
                State::BlockDocStart => {
                    if c == '/' {
                        // `/**/` : empty plain comment
                        self.state = State::Whitespace;
                    } else if c == '*' {
                        // `/***` : plain comment, per Rust convention; the
                        // `*` we saw may begin the closer
                        self.state = State::MaybeEndBlock(0);
                    } else {
                        self.temp.clear();
                        self.temp.push(c);
                        self.state = State::BlockDoc;
                    }
                }
                State::BlockDoc => {
                    if c == '*' {
                        self.state = State::BlockDocMaybeEnd;
                    } else {
                        self.temp.push(c);
                    }
                }
                State::BlockDocMaybeEnd => {
                    if c == '/' {
                        let text = self.temp.trim().to_string();
                        if !text.is_empty() {
                            self.docs.push(ScriptTokDoc {
                                next_token: self.tokens.len() as u32,
                                text,
                            });
                        }
                        self.temp.clear();
                        self.state = State::Whitespace;
                    } else if c == '*' {
                        self.temp.push('*');
                        // stay: this `*` may begin the closer
                    } else {
                        self.temp.push('*');
                        self.temp.push(c);
                        self.state = State::BlockDoc;
                    }
                }
                State::Number => {
                    if self.unit_start.is_some() && c.is_alphabetic() {
                        self.temp.push(c);
                    } else if c.is_numeric() {
                        self.temp.push(c);
                    } else if c == '.' && self.temp.chars().last() == Some('.') {
                        self.temp.pop();
                        self.tok_end = self.lex_start + self.temp.chars().count();
                        self.emit_f64();
                        self.temp.push('.');
                        self.temp.push('.');
                        self.lex_start = self.pos - 2;
                        self.tok_end = self.pos;
                        self.emit_operator();
                        self.state = State::Whitespace
                    } else if c == '.' && self.temp.chars().position(|v| v == '.').is_none() {
                        self.temp.push(c);
                    } else if (c == 'e' || c == 'E')
                        && self
                            .temp
                            .chars()
                            .position(|v| v == 'e' || v == 'E')
                            .is_none()
                    {
                        self.temp.push(c);
                    } else if (c == '+' || c == '-')
                        && matches!(self.temp.chars().last(), Some('e') | Some('E'))
                    {
                        // Handle exponent sign in scientific notation like 1e+20 or 1e-5
                        self.temp.push(c);
                    } else if (c == 'x' || c == 'X')
                        && self
                            .temp
                            .chars()
                            .position(|v| v == 'x' || v == 'X')
                            .is_none()
                    {
                        self.temp.push(c);
                    } else if c == 'f' {
                        self.tok_end = self.pos;
                        self.emit_f32();
                        self.state = State::Whitespace
                    } else if c == 'u' {
                        self.tok_end = self.pos;
                        self.emit_u32();
                        self.state = State::Whitespace
                    } else if c == 'i' {
                        self.tok_end = self.pos;
                        self.emit_i32();
                        self.state = State::Whitespace
                    } else if c == 'h' {
                        self.tok_end = self.pos;
                        self.emit_f16();
                        self.state = State::Whitespace
                    } else if c == '_' {
                        // Numeric separators stay in the literal (parse::<f64>
                        // ignores nothing, so they are simply not pushed).
                        // Moving to Whitespace here left `temp` stale and let a
                        // later separator reach emit_separator and panic.
                        continue;
                    } else if c.is_alphabetic() && !self.temp.contains(['x', 'X']) {
                        // a unit suffix (`0.12s`, `120ms`, `90deg`)
                        self.unit_start = Some(self.temp.len());
                        self.temp.push(c);
                    } else if c == '$' || c.is_alphabetic() {
                        self.emit_f64();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    } else if c == '#' {
                        self.emit_f64();
                        self.state = State::Color;
                        self.temp.push(c);
                    } else if is_operator(c) {
                        self.emit_f64();
                        self.state = State::Operator;
                        self.temp.push(c);
                    } else if is_separator(c) {
                        self.emit_f64();
                        self.emit_separator(c);
                        self.state = State::Whitespace;
                    } else if c == '"' {
                        self.emit_f64();
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.emit_f64();
                        self.state = State::String(false);
                    } else if let Some(tok) = is_block(c) {
                        self.emit_f64();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    } else {
                        self.emit_f64();
                        self.state = State::Whitespace;
                    }
                }
                State::RustValue => {
                    if c >= '0' && c <= '9' {
                        self.temp.push(c);
                    } else {
                        self.emit_rust_value();
                        self.state = State::Whitespace
                    }
                }
                State::Color => {
                    if self.temp.len() == 0 && c == '(' {
                        self.state = State::RustValue
                    } else if c >= '0' && c <= '9' || c >= 'a' && c <= 'f' || c >= 'A' && c <= 'F' {
                        self.temp.push(c);
                        if self.temp.len() == 8 {
                            self.tok_end = self.pos;
                            self.emit_color();
                            self.state = State::Whitespace
                        }
                    } else if c == 'x' && self.temp.len() == 0 { // eat first x
                    } else if c == '_' || c == '$' || c.is_alphabetic() {
                        self.emit_color();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    } else if c == '#' {
                        self.emit_color();
                        self.state = State::Color;
                        self.temp.push(c);
                    } else if is_operator(c) {
                        self.emit_color();
                        self.state = State::Operator;
                        self.temp.push(c);
                    } else if is_separator(c) {
                        self.emit_color();
                        self.emit_separator(c);
                        self.state = State::Whitespace;
                    } else if c == '"' {
                        self.emit_color();
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.emit_color();
                        self.state = State::String(false);
                    } else if let Some(tok) = is_block(c) {
                        self.emit_color();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    } else {
                        self.emit_color();
                        self.state = State::Whitespace;
                    }
                }
            }
            // A token begins at this character when none was being lexed, or
            // when this character ended the one that was.
            if self.state.lexing_token()
                && !was_in_string
                && (!was_lexing || self.tokens.len() != tokens_before)
            {
                self.lex_start = self.pos - 1;
            }
            // Register the newline AFTER this char's token emission, so a token
            // terminated BY this newline keeps whatever preceded it, and the flag
            // attaches to the NEXT token instead.
            if c == '\n' {
                self.newline_pending = true;
            }
            if c.is_whitespace() {
                self.space_pending = true;
            }
        }
        &self.tokens[start..self.tokens.len()]
    }
}

/// One token of a source as the real tokenizer reads it, with the bytes it
/// was written in. Nothing is evaluated or parsed.
#[derive(Clone, Debug)]
pub struct LexedToken {
    pub token: ScriptToken,
    /// Byte range in the source.
    pub span: std::ops::Range<usize>,
    /// A string token's value, escapes resolved.
    pub string: Option<String>,
    pub preceded_by_newline: bool,
    pub preceded_by_space: bool,
}

/// The tokens of `source`, each with its byte range. Comments are not
/// tokens: they stay in the gaps between ranges, so a host that edits by
/// range keeps them. This is the tokenizer a script runs on, so anything
/// the language reads as one token is one token here.
pub fn lex(source: &str) -> Vec<LexedToken> {
    let mut heap = ScriptHeap::default();
    let mut tokenizer = ScriptTokenizer::default();
    tokenizer.tokenize(source, &mut heap);
    tokenizer.finish(&mut heap);
    let mut bytes: Vec<usize> = source.char_indices().map(|(at, _)| at).collect();
    bytes.push(source.len());
    tokenizer
        .tokens
        .iter()
        .map(|pos| {
            let span = pos.span();
            let string = pos.token.as_string().and_then(|value| heap.string_with(value, |_, text| text.to_string()));
            LexedToken {
                token: pos.token,
                span: bytes[span.start.min(bytes.len() - 1)]..bytes[span.end.min(bytes.len() - 1)],
                string,
                preceded_by_newline: pos.preceded_by_newline,
                preceded_by_space: pos.preceded_by_space,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_identifier_does_not_underflow_its_token_position() {
        let mut heap = ScriptHeap::default();
        let mut tokenizer = ScriptTokenizer::default();

        tokenizer.tokenize("\u{540d} ", &mut heap);

        assert_eq!(tokenizer.tokens.len(), 1);
        // Same position convention as an ASCII identifier of the same length.
        let mut ascii = ScriptTokenizer::default();
        ascii.tokenize("a ", &mut heap);
        assert_eq!(
            tokenizer.token_index_to_row_col(0),
            ascii.token_index_to_row_col(0)
        );
    }

    #[test]
    fn token_rows_and_columns_match_a_scan_of_the_source() {
        // Rows and columns come from the line index; they are what a scan
        // of the source up to the token gives, for a source streamed in
        // pieces (a provisional last token in between) too.
        fn scan(source: &str, at: usize) -> Option<(u32, u32)> {
            let (mut line, mut start) = (0, 0);
            for (i, c) in source.chars().enumerate() {
                if i >= at {
                    return Some((line, (i - start) as u32));
                }
                if c == '\n' {
                    start = i + 1;
                    line += 1;
                }
            }
            None
        }
        let mut heap = ScriptHeap::default();
        let mut tokenizer = ScriptTokenizer::default();
        let source = "let a = 1\n\n  b: \u{540d}+ 2\nc(3, 4)\n   x";
        for piece in ["let a = 1\n", "\n  b: \u{540d}", "+ 2\nc(3, 4)\n   x"] {
            tokenizer.tokenize(piece, &mut heap);
            tokenizer.push_pending_token(&mut heap);
        }
        assert!(tokenizer.tokens.len() > 8);
        for (i, token) in tokenizer.tokens.iter().enumerate() {
            assert_eq!(tokenizer.token_index_to_row_col(i as u32), scan(source, token.pos), "token {i}");
        }
    }

    #[test]
    fn numeric_underscores_stay_in_the_pending_number_until_a_separator() {
        let mut heap = ScriptHeap::default();
        let mut tokenizer = ScriptTokenizer::default();

        tokenizer.tokenize("0_;1_000;1.5_;1e2_;", &mut heap);

        assert_eq!(tokenizer.tokens.len(), 8);
        assert_eq!(tokenizer.tokens[0].token.as_u40(), Some(0));
        assert_eq!(tokenizer.tokens[2].token.as_u40(), Some(1_000));
        assert_eq!(tokenizer.tokens[4].token.as_f64(), Some(1.5));
        assert_eq!(tokenizer.tokens[6].token.as_f64(), Some(100.0));
        for separator in [1, 3, 5, 7] {
            assert!(matches!(
                tokenizer.tokens[separator].token,
                ScriptToken::Separator(_)
            ));
        }
    }

    #[test]
    fn numeric_underscore_before_whitespace_flushes_through_the_number_path() {
        // The old tokenizer moved to Whitespace on `_` while keeping the
        // buffered digits; the terminal preflight marker `\n;` then reached
        // emit_separator with stale text and panicked.
        let mut heap = ScriptHeap::default();
        let mut tokenizer = ScriptTokenizer::default();

        tokenizer.tokenize("1_000", &mut heap);
        tokenizer.tokenize("\n;", &mut heap);

        assert_eq!(tokenizer.tokens.len(), 2);
        assert_eq!(tokenizer.tokens[0].token.as_u40(), Some(1_000));
        assert!(matches!(tokenizer.tokens[1].token, ScriptToken::Separator(_)));

        let mut tokenizer = ScriptTokenizer::default();
        tokenizer.tokenize("1_ 2;", &mut heap);
        assert_eq!(tokenizer.tokens.len(), 3);
        assert_eq!(tokenizer.tokens[0].token.as_u40(), Some(1));
        assert_eq!(tokenizer.tokens[1].token.as_u40(), Some(2));
        assert!(matches!(tokenizer.tokens[2].token, ScriptToken::Separator(_)));
    }

    #[test]
    fn tokens_carry_the_exact_characters_they_were_written_in() {
        let source = "Rect{a:-1.5 b: @ease_out c: #ff5a36, d: \"h\\\"i\" // note\n e: vec2(1..2) f: 1.0f g: x->y}\n";
        let mut heap = ScriptHeap::default();
        let mut tokenizer = ScriptTokenizer::default();
        tokenizer.tokenize(source, &mut heap);
        tokenizer.finish(&mut heap);
        let chars: Vec<char> = source.chars().collect();
        let texts: Vec<String> = tokenizer
            .tokens
            .iter()
            .map(|token| chars[token.span()].iter().collect())
            .collect();
        assert_eq!(
            texts,
            [
                "Rect", "{", "a", ":", "-", "1.5", "b", ":", "@", "ease_out", "c", ":", "#ff5a36", ",", "d", ":",
                "\"h\\\"i\"", "e", ":", "vec2", "(", "1", "..", "2", ")", "f", ":", "1.0f", "g", ":", "x", "->", "y", "}"
            ]
        );
    }
}
