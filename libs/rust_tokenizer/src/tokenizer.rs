//! This module contains code for tokenizing Rust code.
//! 
//! The tokenizer in this module supports lazy tokenization. That is, it has an explicit state,
//! which can be recorded at the start of each line. Running the tokenizer with the same starting
//! state on the same line will always result in the same sequence of tokens. This means that if
//! neither the contents nor the starting state of the tokenizer changed for a given line, that
//! line does not need to be retokenized. 
//! 
//! The tokenizer consumes one token at a time. The only exception to this are multiline tokens,
//! such as comments and strings, which are broken up into separate tokens for each line.
//! Consequently, the only time the tokenizer can end up in a state other than the initial state is
//! when it is in the middle of tokenizing a multiline token and runs into the end of the line
//! before it finds the end of the token.
#[allow(clippy::collapsible_if)]

use {
    crate::{
        full_token::{TokenWithLen, Delim, FullToken},
        colorhex,
    },
    makepad_live_id::LiveId
};

/// The state of the tokenizer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Initial(InitialState),
    BlockCommentTail(BlockCommentTailState),
    DoubleQuotedStringTail(DoubleQuotedStringTailState),
    RawDoubleQuotedStringTail(RawDoubleQuotedStringTailState),
}

impl Default for State {
    fn default() -> State {
        State::Initial(InitialState)
    }
}

impl State {
    /// Given the current state of the tokenizer and a cursor over a slice of chars, finds the next
    /// token in in that string, and moves the cursor forward by the number of characters in the
    /// token. Returns the new state of the tokenizer and the token recognised, or `None` if there
    /// are no more tokens in the string.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_rust_tokenizer::{
    ///     full_token::{FullToken, TokenWithLen},
    ///     tokenizer::{Cursor, InitialState, State}
    /// };
    /// 
    /// let mut state = State::default();
    /// let mut scratch = String::new();
    /// let mut cursor = Cursor::new(&['1', '2', '3'], &mut scratch);
    /// assert_eq!(
    ///     state.next(&mut cursor),
    ///     (
    ///         State::Initial(InitialState),
    ///         Some(TokenWithLen {
    ///            len: 3,
    ///            token: FullToken::Int(123),
    ///         })
    ///     )
    /// );
    /// ```
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<TokenWithLen>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, token) = match self {
            State::Initial(state) => state.next(cursor),
            State::BlockCommentTail(state) => state.next(cursor),
            State::DoubleQuotedStringTail(state) => state.next(cursor),
            State::RawDoubleQuotedStringTail(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (
            next_state,
            Some(TokenWithLen {
                len: end - start,
                token,
            }),
        )
    }
}

/// The state of the tokenizer when it is not in the middle of any token.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InitialState;

#[allow(clippy::collapsible_if)]
impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, FullToken) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('r', '#', '"') | ('r', '#', '#') | ('r', '"', _) => self.raw_string(cursor),
            ('r', '#', ch2) if ch2.is_identifier_start() => self.raw_identifier(cursor),
            ('b', 'r', '"') | ('b', 'r', '#') => self.raw_byte_string(cursor),
            ('.', '.', '.') | ('.', '.', '=') | ('<', '<', '=') | ('>', '>', '=') => {
                let id = cursor.id_from_3();
                cursor.skip(3);
                (
                    State::Initial(InitialState),
                    FullToken::Punct(id),
                )
            }
            ('/', '/', _) => self.line_comment(cursor),
            ('/', '*', _) => self.block_comment(cursor),
            ('b', '\'', _) => self.byte(cursor),
            ('b', '"', _) => self.byte_string(cursor),
            ('!', '=', _)
                | ('%', '=', _)
                | ('&', '&', _)
                | ('&', '=', _)
                | ('*', '=', _)
                | ('+', '=', _)
                | ('-', '=', _)
                | ('-', '>', _)
                | ('.', '.', _)
                | ('/', '=', _)
                | (':', ':', _)
                | ('<', '<', _)
                | ('<', '=', _)
                | ('=', '=', _)
                | ('=', '>', _)
                | ('>', '=', _)
                | ('>', '>', _)
                | ('^', '=', _)
                | ('|', '=', _)
                | ('|', '|', _) => {
                let id = cursor.id_from_2();
                cursor.skip(2);
                (
                    State::Initial(InitialState),
                    FullToken::Punct(id),
                )
            }
            ('\'', _, _) => self.char_or_lifetime(cursor),
            ('"', _, _) => self.string(cursor),
            ('(', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    FullToken::Open(Delim::Paren),
                )
            }
            (')', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    FullToken::Close(Delim::Paren),
                )
            }
            ('[', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    FullToken::Open(Delim::Bracket),
                )
            }
            (']', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    FullToken::Close(Delim::Bracket),
                )
            }
            ('{', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    FullToken::Open(Delim::Brace),
                )
            }
            ('}', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    FullToken::Close(Delim::Brace),
                )
            }
            ('#', ch1, ch2) if ch1 == 'x' && ch2.is_digit(16) || ch1.is_digit(16) => self.color(cursor),
            ('_', ch1, _) if ch1.is_identifier_continue() => self.identifier_or_bool(cursor),
            ('!', _, _)
                | ('#', _, _)
                | ('$', _, _)
                | ('%', _, _)
                | ('&', _, _)
                | ('*', _, _)
                | ('+', _, _)
                | (',', _, _)
                | ('-', _, _)
                | ('.', _, _)
                | ('/', _, _)
                | (':', _, _)
                | (';', _, _)
                | ('<', _, _)
                | ('=', _, _)
                | ('>', _, _)
                | ('?', _, _)
                | ('@', _, _)
                | ('^', _, _)
                | ('_', _, _)
                | ('|', _, _) => {
                let id = cursor.id_from_1();
                 cursor.skip(1);
                (
                    State::Initial(InitialState),
                    FullToken::Punct(id),
                )
            }
            (ch, _, _) if ch.is_identifier_start() => self.identifier_or_bool(cursor),
            (ch, _, _) if ch.is_digit(10) => self.number(cursor),
            (ch, _, _) if ch.is_whitespace() => self.whitespace(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), FullToken::Unknown)
            }
        }
    }
    
    fn line_comment(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == '/' && cursor.peek(1) == '/');
        cursor.skip(2);
        while cursor.skip_if( | ch | ch != '\0') {}
        (State::Initial(InitialState), FullToken::Comment)
    }
    
    fn block_comment(self, cursor: &mut Cursor<'_>) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == '/' && cursor.peek(1) == '*');
        cursor.skip(2);
        BlockCommentTailState {depth: 0}.next(cursor)
    }
    
    fn identifier_or_bool(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        let start = cursor.index();
        match cursor.peek(0) {
            'f' => {
                cursor.skip(1);
                if "alse".chars().all(|expected| cursor.skip_if(|actual| actual == expected)) {
                    if !cursor.peek(0).is_identifier_continue() {
                        return (State::Initial(InitialState), FullToken::Bool(false));
                    }
                }
                self.identifier_tail(start, cursor)
            }
            't' => {
                cursor.skip(1);
                if "rue".chars().all(|expected| cursor.skip_if(|actual| actual == expected)) {
                    if !cursor.peek(0).is_identifier_continue() {
                        return (State::Initial(InitialState), FullToken::Bool(true));
                    }
                }
                self.identifier_tail(start, cursor)
            },
            _ => self.identifier_tail(start, cursor),
        }
    }
    
    fn identifier_tail(self, start: usize, cursor: &mut Cursor) -> (State, FullToken) {
        while cursor.skip_if( | ch | ch.is_identifier_continue()) {}
        (State::Initial(InitialState), FullToken::Ident(
            LiveId::from_str(cursor.from_start_to_scratch(start))
        ))
    }

    /// `r#ident`: one identifier token including its prefix.
    fn raw_identifier(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == 'r' && cursor.peek(1) == '#');
        let start = cursor.index();
        cursor.skip(2);
        self.identifier_tail(start, cursor)
    }
    
    fn number(self, cursor: &mut Cursor) -> (State, FullToken) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), FullToken::Unknown);
                }
                return (State::Initial(InitialState), FullToken::OtherNumber)
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), FullToken::Unknown);
                }
                return (State::Initial(InitialState), FullToken::OtherNumber)
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), FullToken::Unknown);
                }
                return (State::Initial(InitialState), FullToken::OtherNumber)
            }
            _ => {
                let start = cursor.index();
                // normal number
                cursor.skip_digits(10);
                
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(1).is_identifier_start() => {
                        cursor.skip(1);
                        if cursor.skip_digits(10) {
                            if cursor.peek(0) == 'E' || cursor.peek(0) == 'e' {
                                if !cursor.skip_exponent() {
                                    return (State::Initial(InitialState), FullToken::Unknown);
                                }
                            }
                        }
                        if cursor.skip_suffix() {
                            return (State::Initial(InitialState), FullToken::OtherNumber)
                        }
                        // parse as float
                        if let Ok(value) = cursor.from_start_to_scratch(start).parse::<f64>() {
                            return (State::Initial(InitialState), FullToken::Float(value))
                        }
                        else {
                            return (State::Initial(InitialState), FullToken::Unknown)
                        }
                    }
                    'E' | 'e' => {
                        if !cursor.skip_exponent() {
                            return (State::Initial(InitialState), FullToken::Unknown);
                        }
                        if cursor.skip_suffix() {
                            return (State::Initial(InitialState), FullToken::OtherNumber)
                        }
                        // parse as float
                        if let Ok(value) = cursor.from_start_to_scratch(start).parse::<f64>() {
                            return (State::Initial(InitialState), FullToken::Float(value))
                        }
                        else {
                            return (State::Initial(InitialState), FullToken::Unknown)
                        }
                    }
                    _ => {
                        if cursor.skip_suffix() {
                            return (State::Initial(InitialState), FullToken::OtherNumber)
                        }
                        // normal number
                        if let Ok(value) = cursor.from_start_to_scratch(start).parse::<i64>() {
                            return (State::Initial(InitialState), FullToken::Int(value))
                        }
                        else {
                            return (State::Initial(InitialState), FullToken::Unknown)
                        }
                    }
                }
            }
        };
    }
    
    fn color(self, cursor: &mut Cursor) -> (State, FullToken) {
        let start = match (cursor.peek(0), cursor.peek(1)) {
            ('#', 'x') => {
                cursor.skip(2);
                let start = cursor.index();
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), FullToken::Unknown);
                }
                start
            }
            _ => {
                cursor.skip(1);
                let start = cursor.index();
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), FullToken::Unknown);
                }
                start
            }
        };
        if let Ok(col) = colorhex::hex_bytes_to_u32(cursor.from_start_to_scratch(start).as_bytes()) {
            (State::Initial(InitialState), FullToken::Color(col))
        }
        else {
            (State::Initial(InitialState), FullToken::Unknown)
        }
    }
    
    fn char_or_lifetime(self, cursor: &mut Cursor) -> (State, FullToken) {
        if cursor.peek(1).is_identifier_start() && cursor.peek(2) != '\'' {
            debug_assert!(cursor.peek(0) == '\'');
            cursor.skip(2);
            while cursor.skip_if( | ch | ch.is_identifier_continue()) {}
            if cursor.peek(0) == '\'' {
                cursor.skip(1);
                cursor.skip_suffix();
                (State::Initial(InitialState), FullToken::String)
            } else {
                (State::Initial(InitialState), FullToken::Lifetime)
            }
        } else {
            self.single_quoted_string(cursor)
        }
    }
    
    fn byte(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == 'b');
        cursor.skip(1);
        self.single_quoted_string(cursor)
    }
    
    fn string(self, cursor: &mut Cursor) -> (State, FullToken) {
        self.double_quoted_string(cursor)
    }
    
    fn byte_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == 'b');
        cursor.skip(1);
        self.double_quoted_string(cursor)
    }
    
    fn raw_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == 'r');
        cursor.skip(1);
        self.raw_double_quoted_string(cursor)
    }
    
    fn raw_byte_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == 'b' && cursor.peek(1) == 'r');
        cursor.skip(2);
        self.raw_double_quoted_string(cursor)
    }
    
    fn single_quoted_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == '\'');
        cursor.skip(1);
        loop {
            match (cursor.peek(0), cursor.peek(1)) {
                ('\'', _) => {
                    cursor.skip(1);
                    cursor.skip_suffix();
                    break;
                }
                ('\0', _) => return (State::Initial(InitialState), FullToken::Unknown),
                ('\\', '\0') => cursor.skip(1),
                ('\\', _) => cursor.skip(2),
                _ => cursor.skip(1),
            }
        }
        (State::Initial(InitialState), FullToken::String)
    }
    
    fn double_quoted_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == '"');
        cursor.skip(1);
        DoubleQuotedStringTailState.next(cursor)
    }
    
    fn raw_double_quoted_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        let mut start_hash_count = 0;
        while cursor.skip_if( | ch | ch == '#') {
            start_hash_count += 1;
        }
        // The opening quote belongs to the opener, never to the body.
        if !cursor.skip_if( | ch | ch == '"') {
            return (State::Initial(InitialState), FullToken::Unknown);
        }
        RawDoubleQuotedStringTailState {start_hash_count}.next(cursor)
    }
    
    fn whitespace(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if( | ch | ch.is_whitespace()) {}
        (State::Initial(InitialState), FullToken::Whitespace)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BlockCommentTailState {
    depth: usize,
}

impl BlockCommentTailState {
    /// A state inside a block comment nested `depth` levels deep.
    pub fn new(depth: usize) -> Self {
        BlockCommentTailState { depth }
    }
    pub fn depth(&self) -> usize {
        self.depth
    }
}

impl BlockCommentTailState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, FullToken) {
        let mut state = self;
        loop {
            match (cursor.peek(0), cursor.peek(1)) {
                ('/', '*') => {
                    cursor.skip(2);
                    state.depth += 1;
                }
                ('*', '/') => {
                    cursor.skip(2);
                    if state.depth == 0 {
                        break (State::Initial(InitialState), FullToken::Comment);
                    }
                    state.depth -= 1;
                }
                ('\0', _) => {
                    break (State::BlockCommentTail(state), FullToken::Comment);
                }
                _ => cursor.skip(1),
            }
        }
    }
}

/// The state of the tokenizer when it is in the middle of a double quoted string.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DoubleQuotedStringTailState;

impl DoubleQuotedStringTailState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, FullToken) {
        loop {
            match (cursor.peek(0), cursor.peek(1)) {
                ('"', _) => {
                    cursor.skip(1);
                    cursor.skip_suffix();
                    break (State::Initial(InitialState), FullToken::String);
                }
                ('\0', _) => {
                    break (
                        State::DoubleQuotedStringTail(DoubleQuotedStringTailState),
                        FullToken::String,
                    );
                }
                ('\\', '\0') => cursor.skip(1),
                ('\\', _) => cursor.skip(2),
                _ => cursor.skip(1),
            }
        }
    }
}

/// The state of the tokenizer when it is in the middle of a raw double quoted string.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RawDoubleQuotedStringTailState {
    start_hash_count: usize,
}

impl RawDoubleQuotedStringTailState {
    /// A state inside a raw string opened with `start_hash_count` hashes.
    pub fn new(start_hash_count: usize) -> Self {
        RawDoubleQuotedStringTailState { start_hash_count }
    }
    pub fn start_hash_count(&self) -> usize {
        self.start_hash_count
    }
}

impl RawDoubleQuotedStringTailState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, FullToken) {
        loop {
            match cursor.peek(0) {
                '"' => {
                    cursor.skip(1);
                    let mut end_hash_count = 0;
                    while end_hash_count < self.start_hash_count && cursor.skip_if( | ch | ch == '#') {
                        end_hash_count += 1;
                    }
                    if end_hash_count == self.start_hash_count {
                        cursor.skip_suffix();
                        break (State::Initial(InitialState), FullToken::String);
                    }
                }
                '\0' => {
                    break (State::RawDoubleQuotedStringTail(self), FullToken::String);
                }
                _ => cursor.skip(1),
            }
        }
    }
}

/// A cursor over a slice of chars.
#[derive(Debug)]
pub struct Cursor<'a> {
    chars: &'a [char],
    scratch: &'a mut String,
    index: usize,
}

impl<'a> Cursor<'a> {
    /// Creates a cursor over a slice of chars. The `scratch` parameter provides scratch storage for
    /// building a string when necessary.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_rust_tokenizer::tokenizer::Cursor;
    /// 
    /// let mut scratch = String::new();
    /// let cursor = Cursor::new(&['1', '2', '3'], &mut scratch);
    /// ```
    pub fn new(chars: &'a [char], scratch: &'a mut String) -> Cursor<'a> {
        Cursor {chars, scratch, index: 0 }
    }
    
    fn index(&self) -> usize {
        self.index
    }
    
    fn from_start_to_scratch(&mut self, start: usize) -> &str {
        self.scratch.clear();
        for i in start..self.index {
            self.scratch.push(self.chars[i]);
        }
        &self.scratch
    }
    
    
    fn peek(&self, index: usize) -> char {
        self.chars.get(self.index + index).cloned().unwrap_or('\0')
    }
    
    fn id_from_1(&self) -> LiveId {
        LiveId::from_bytes(LiveId::SEED, &[
            self.chars[self.index + 0] as u8,
        ], 0, 1, 0)
    }
    
    fn id_from_2(&self) -> LiveId {
        LiveId::from_bytes(LiveId::SEED, &[
            self.chars[self.index + 0] as u8,
            self.chars[self.index + 1] as u8,
        ], 0, 2, 0)
    }
    
    fn id_from_3(&self) -> LiveId {
        LiveId::from_bytes(LiveId::SEED, &[
            self.chars[self.index + 0] as u8,
            self.chars[self.index + 1] as u8,
            self.chars[self.index + 2] as u8,
        ], 0, 3, 0)
    }
    
    fn skip(&mut self, count: usize) {
        self.index += count;
    }
    
    fn skip_if<P>(&mut self, predicate: P) -> bool
    where
    P: FnOnce(char) -> bool,
    {
        if predicate(self.peek(0)) {
            self.skip(1);
            true
        } else {
            false
        }
    }
    
    fn skip_exponent(&mut self) -> bool {
        debug_assert!(self.peek(0) == 'E' || self.peek(0) == 'e');
        self.skip(1);
        if self.peek(0) == '+' || self.peek(0) == '-' {
            self.skip(1);
        }
        self.skip_digits(10)
    }
    
    fn skip_digits(&mut self, radix: u32) -> bool {
        let mut has_skip_digits = false;
        loop {
            match self.peek(0) {
                '_' => {
                    self.skip(1);
                }
                ch if ch.is_digit(radix) => {
                    self.skip(1);
                    has_skip_digits = true;
                }
                _ => break,
            }
        }
        has_skip_digits
    }
    
    fn skip_suffix(&mut self) -> bool {
        if self.peek(0).is_identifier_start() {
            self.skip(1);
            while self.skip_if( | ch | ch.is_identifier_continue()) {}
            return true
        }
        false
    }
}


#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TokenPos {
    pub line: usize,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TokenRange {
    pub start: TokenPos,
    pub end: TokenPos
}

impl TokenRange{
    pub fn is_in_range(&self, pos:TokenPos)->bool{
        if self.start.line == self.end.line{
            pos.line == self.start.line && pos.index >= self.start.index && pos.index < self.end.index
        }
        else{
            pos.line == self.start.line && pos.index >= self.start.index ||
            pos.line > self.start.line && pos.line < self.end.line ||
            pos.line == self.end.line && pos.index < self.end.index
        }
    }
}

/// Extension methods for `char`.
///
/// Identifier characters follow a practical reading of Unicode Standard Annex #31: ASCII
/// letters, digits and `_`, plus every non-ASCII alphabetic (start) or alphanumeric
/// (continue) character. Full XID tables are not shipped; the rare characters that XID
/// admits but `char::is_alphabetic` does not (some marks and connectors) lex as `Unknown`
/// and are retained as such, never dropped.
pub trait CharExt {
    /// Checks if `char` is the start of an identifier.
    fn is_identifier_start(self) -> bool;

    /// Checks if `char` is the continuation of an identifier.
    fn is_identifier_continue(self) -> bool;
}

impl CharExt for char {
    fn is_identifier_start(self) -> bool {
        self == '_' || self.is_ascii_alphabetic() || (!self.is_ascii() && self.is_alphabetic())
    }

    fn is_identifier_continue(self) -> bool {
        self == '_' || self.is_ascii_alphanumeric() || (!self.is_ascii() && self.is_alphanumeric())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<(FullToken, String)> {
        let mut state = State::default();
        let mut scratch = String::new();
        let mut out = Vec::new();
        for line in src.split('\n') {
            let chars: Vec<char> = line.chars().collect();
            let mut cursor = Cursor::new(&chars, &mut scratch);
            let mut at = 0;
            loop {
                let (next, token) = state.next(&mut cursor);
                state = next;
                let Some(token) = token else { break };
                assert!(at + token.len <= chars.len(), "token overran the line in {src:?}");
                let text: String = chars[at..at + token.len].iter().collect();
                at += token.len;
                if !matches!(token.token, FullToken::Whitespace) {
                    out.push((token.token, text));
                }
            }
            assert_eq!(at, chars.len(), "tokens must cover the line in {src:?}");
        }
        out
    }

    fn end_state(src: &str) -> State {
        let chars: Vec<char> = src.chars().collect();
        let mut scratch = String::new();
        let mut cursor = Cursor::new(&chars, &mut scratch);
        let mut state = State::default();
        loop {
            let (next, token) = state.next(&mut cursor);
            state = next;
            if token.is_none() {
                break state;
            }
        }
    }

    fn texts(src: &str) -> Vec<String> {
        toks(src).into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn raw_strings_all_hash_counts() {
        assert_eq!(texts(r#"r"a\" b"#), vec![r#"r"a\""#, "b"]);
        assert_eq!(texts(r##"r#"x"y"# z"##), vec![r##"r#"x"y"#"##, "z"]);
        assert_eq!(texts(r###"r##"x"#y"## z"###), vec![r###"r##"x"#y"##"###, "z"]);
        assert_eq!(texts(r#"br"bytes" q"#), vec![r#"br"bytes""#, "q"]);
        assert_eq!(texts(r##"br#"b"# q"##), vec![r##"br#"b"#"##, "q"]);
        assert!(matches!(toks(r#"r"a\" b"#)[0].0, FullToken::String));
        // an opener without its quote is unknown, not a string that eats the line
        assert_eq!(texts("r## x"), vec!["r##", "x"]);
        assert!(matches!(toks("r## x")[0].0, FullToken::Unknown));
    }

    #[test]
    fn escapes_do_not_swallow_the_closing_quote() {
        assert_eq!(texts(r#""a\\"; let"#), vec![r#""a\\""#, ";", "let"]);
        assert_eq!(texts(r#""q\"x" y"#), vec![r#""q\"x""#, "y"]);
        assert_eq!(texts(r#"'\\'; x"#), vec![r#"'\\'"#, ";", "x"]);
        assert_eq!(texts(r#"'\''; x"#), vec![r#"'\''"#, ";", "x"]);
        assert_eq!(texts(r#"b'\\' x"#), vec![r#"b'\\'"#, "x"]);
        assert!(matches!(end_state(r#""open\"#), State::DoubleQuotedStringTail(_)));
        assert!(matches!(end_state(r#""closed\\""#), State::Initial(_)));
        assert!(matches!(end_state(r#""trailing backslash \"#), State::DoubleQuotedStringTail(_)));
    }

    #[test]
    fn raw_identifiers_lifetimes_and_chars() {
        assert_eq!(texts("r#type x"), vec!["r#type", "x"]);
        assert!(matches!(toks("r#type")[0].0, FullToken::Ident(_)));
        assert_eq!(texts("'a'"), vec!["'a'"]);
        assert!(matches!(toks("'a'")[0].0, FullToken::String));
        assert_eq!(texts("<'a>"), vec!["<", "'a", ">"]);
        assert!(matches!(toks("<'a>")[1].0, FullToken::Lifetime));
        assert_eq!(texts("'static x"), vec!["'static", "x"]);
        assert_eq!(texts(r#"'\n' x"#), vec![r#"'\n'"#, "x"]);
        assert_eq!(texts(r#"'\u{1F600}' x"#), vec![r#"'\u{1F600}'"#, "x"]);
    }

    #[test]
    fn unicode_identifiers_and_nested_comments() {
        assert_eq!(texts("let é = ünï;"), vec!["let", "é", "=", "ünï", ";"]);
        assert!(matches!(toks("é")[0].0, FullToken::Ident(_)));
        assert_eq!(texts("/* a /* b */ c */ d"), vec!["/* a /* b */ c */", "d"]);
        assert!(matches!(end_state("/* open /* nested */"), State::BlockCommentTail(_)));
        assert!(matches!(end_state("/* open /* nested */ */"), State::Initial(_)));
    }

    #[test]
    fn underscore_identifiers() {
        assert_eq!(texts("struct _Hidden; let _ = _x + __y;"), vec!["struct", "_Hidden", ";", "let", "_", "=", "_x", "+", "__y", ";"]);
        assert!(matches!(toks("_Hidden")[0].0, FullToken::Ident(_)));
        assert!(matches!(toks("_")[0].0, FullToken::Punct(_)));
        assert!(matches!(toks("_1")[0].0, FullToken::Ident(_)));
    }

    #[test]
    fn numbers_and_field_access() {
        assert_eq!(texts("x.0.1"), vec!["x", ".", "0.1"]);
        assert_eq!(texts("x.0.len()"), vec!["x", ".", "0", ".", "len", "(", ")"]);
        assert_eq!(texts("1.max(2)"), vec!["1", ".", "max", "(", "2", ")"]);
        assert_eq!(texts("1..2"), vec!["1", "..", "2"]);
        assert_eq!(texts("1.5f32 + 2u8 + 0x1F + 1e3"), vec!["1.5f32", "+", "2u8", "+", "0x1F", "+", "1e3"]);
        assert!(matches!(toks("1.5")[0].0, FullToken::Float(_)));
        assert!(matches!(toks("1.max(2)")[0].0, FullToken::Int(1)));
    }

    #[test]
    fn identifiers_do_not_grow_the_global_table() {
        let a = toks("some_identifier_name")[0].0.clone();
        let b = toks("some_identifier_name")[0].0.clone();
        assert_eq!(a, b);
        assert!(matches!(a, FullToken::Ident(id) if id == LiveId::from_str("some_identifier_name")));
    }
}
