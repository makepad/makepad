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

use {
    std::rc::Rc,
    crate::{
        char_ext::CharExt,
        live_id::{LiveId,LIVE_ID_SEED},
        full_token::{TokenWithLen, Delim, FullToken},
        colorhex
    },
};

/// The state of the tokenizer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Initial(InitialState),
    BlockCommentTail(BlockCommentTailState),
    DoubleQuotedStringTail(DoubleQuotedStringTailState),
    //DoubleQuotedDependencyTailState(DoubleQuotedDependencyTailState),
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
    /// use makepad_live_tokenizer::{
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
            //State::DoubleQuotedDependencyTailState(state)=> state.next(cursor)
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

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, FullToken) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('r', '#', '"') | ('r', '#', '#') => self.raw_string(cursor),
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
            //('b', '\'', _) => self.byte(cursor),
            ('b', '"', _) => self.byte_string(cursor),
            //('d', '"', _) => self.dependency_string(cursor),
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
            //('\'', _, _) => self.char_or_lifetime(cursor),
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
            ('#', ch1, ch2) if ch1 == 'x' && ch2.is_ascii_hexdigit() || ch1.is_ascii_hexdigit() => self.color(cursor),
            ('.', ch1, _) if ch1.is_ascii_digit() => self.number(cursor),
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
               // | ('_', _, _)
                | ('|', _, _) => {
                let id = cursor.id_from_1();
                 cursor.skip(1);
                (
                    State::Initial(InitialState),
                    FullToken::Punct(id),
                )
            }
            (ch, _, _) if ch.is_identifier_start() => self.identifier_or_bool(cursor),
            (ch, _, _) if ch.is_ascii_digit() => self.number(cursor),
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
        while cursor.skip_if( | ch | ch != '\n' && ch != '\0') {}
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
                if "alse".chars().all( | expected | cursor.skip_if( | actual | actual == expected)) && !cursor.peek(0).is_identifier_continue() {
                    return (State::Initial(InitialState), FullToken::Bool(false));
                }
                self.identifier_tail(start, cursor)
            }
            't' => {
                cursor.skip(1);
                if "rue".chars().all( | expected | cursor.skip_if( | actual | actual == expected)) && !cursor.peek(0).is_identifier_continue() {
                    return (State::Initial(InitialState), FullToken::Bool(true));
                }
                self.identifier_tail(start, cursor)
            },
            _ => self.identifier_tail(start, cursor),
        }
    }
    
    fn identifier_tail(self, start: usize, cursor: &mut Cursor) -> (State, FullToken) {
        while cursor.skip_if( | ch | ch.is_identifier_continue()) {}
        (State::Initial(InitialState), FullToken::Ident(
            LiveId::from_str_with_lut(cursor.from_start_to_scratch(start)).unwrap()
        ))
    }
    
    fn number(self, cursor: &mut Cursor) -> (State, FullToken) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), FullToken::Unknown);
                }
                (State::Initial(InitialState), FullToken::OtherNumber)
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), FullToken::Unknown);
                }
                (State::Initial(InitialState), FullToken::OtherNumber)
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), FullToken::Unknown);
                }
                (State::Initial(InitialState), FullToken::OtherNumber)
            }
            _ => {
                let start = cursor.index();
                // normal number
                cursor.skip_digits(10);
                
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
                        cursor.skip(1);
                        if cursor.skip_digits(10) && (cursor.peek(0) == 'E' || cursor.peek(0) == 'e') && !cursor.skip_exponent() {
                            return (State::Initial(InitialState), FullToken::Unknown);
                        }
                        if cursor.skip_suffix() {
                            return (State::Initial(InitialState), FullToken::OtherNumber)
                        }
                        // parse as float
                        if let Ok(value) = cursor.from_start_to_scratch(start).parse::<f64>() {
                            (State::Initial(InitialState), FullToken::Float(value))
                        }
                        else {
                            (State::Initial(InitialState), FullToken::Unknown)
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
                            (State::Initial(InitialState), FullToken::Float(value))
                        }
                        else {
                            (State::Initial(InitialState), FullToken::Unknown)
                        }
                    }
                    _ => {
                        if cursor.skip_suffix() {
                            return (State::Initial(InitialState), FullToken::OtherNumber)
                        }
                        // normal number
                        if let Ok(value) = cursor.from_start_to_scratch(start).parse::<i64>() {
                            (State::Initial(InitialState), FullToken::Int(value))
                        }
                        else {
                            (State::Initial(InitialState), FullToken::Unknown)
                        }
                    }
                }
            }
        }
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
    /*
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
    }*/
    
    /*fn byte(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == 'b');
        cursor.skip(1);
        self.single_quoted_string(cursor)
    }*/
    
    fn string(self, cursor: &mut Cursor) -> (State, FullToken) {
        self.double_quoted_string(cursor)
    }

    /*fn dependency_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == 'd');
        cursor.skip(1);
        self.double_quoted_dependency(cursor)
    }
    */
    
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
    /*
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
                ('\\', '\'') | ('\\', '\\') => cursor.skip(2),
                _ => cursor.skip(1),
            }
        }
        (State::Initial(InitialState), FullToken::String)
    }*/
    
    fn double_quoted_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == '"');
        cursor.skip(1);
        DoubleQuotedStringTailState.next(cursor)
    }
/*
    fn double_quoted_dependency(self, cursor: &mut Cursor) -> (State, FullToken) {
        debug_assert!(cursor.peek(0) == '"');
        cursor.skip(1);
        DoubleQuotedDependencyTailState.next(cursor)
    }*/
    
    fn raw_double_quoted_string(self, cursor: &mut Cursor) -> (State, FullToken) {
        let mut start_hash_count = 0;
        while cursor.skip_if( | ch | ch == '#') {
            start_hash_count += 1;
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
        let mut s = String::new();
        enum Skip{
            Scanning(bool, usize, usize),
            Found(usize)
        }
        let mut skip = Skip::Scanning(true, 0,0);
        loop {
            match (cursor.peek(0), cursor.peek(1)) {
                ('"', _) => {
                    cursor.skip(1);
                    cursor.skip_suffix();
                    break (State::Initial(InitialState), FullToken::String(Rc::new(s)));
                }
                ('\0', _) => {
                    break (
                        State::DoubleQuotedStringTail(DoubleQuotedStringTailState),
                        FullToken::String(Rc::new(s)),
                    );
                }
                ('\\', '\\') => {
                    if let Skip::Scanning(_,_,len) = skip{
                        skip = Skip::Found(len);
                    }
                    s.push('\\');
                    cursor.skip(2);
                },
                ('\\', '"') => {
                    if let Skip::Scanning(_,_,len) = skip{
                        skip = Skip::Found(len);
                    }
                    s.push('"');
                    cursor.skip(2);
                },
                ('\\', 'n') => {
                    if let Skip::Scanning(_,_,len) = skip{
                        skip = Skip::Found(len);
                    }
                    s.push('\n');
                    cursor.skip(2);
                },
                ('\n',_)=>{ // first newline sets indent strip
                    s.push('\n');
                    if let Skip::Scanning(first,_,len) = skip{
                        skip = Skip::Scanning(first, 0, len);
                    }
                    else if let Skip::Found(len) = skip{
                        skip = Skip::Scanning(false, 0, len);
                    }
                    cursor.skip(1);
                }
                (' ', _)=>{
                    if let Skip::Scanning(first, count, len) = &mut skip{
                        if *first{
                            *len += 1;
                        }
                        else{
                            if *count>=*len{
                                skip = Skip::Found(*len);
                                s.push(' ');
                            }
                            else{
                                *count += 1;
                            }
                        }
                    }
                    else{
                        s.push(' ');
                    }
                    cursor.skip(1);
                }
                (x,_) => {
                    if let Skip::Scanning(_,_,len) = skip{
                        skip = Skip::Found(len);
                    }
                    s.push(x);
                    cursor.skip(1);
                }
            }
        }
    }
}
/*
/// The state of the tokenizer when it is in the middle of a double quoted string.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DoubleQuotedDependencyTailState;

impl DoubleQuotedDependencyTailState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, FullToken) {
        loop {
            match (cursor.peek(0), cursor.peek(1)) {
                ('"', _) => {
                    cursor.skip(1);
                    cursor.skip_suffix();
                    break (State::Initial(InitialState), FullToken::Dependency);
                }
                ('\0', _) => {
                    break (
                        State::DoubleQuotedDependencyTailState(DoubleQuotedDependencyTailState),
                        FullToken::Dependency,
                    );
                }
                ('\\', '"') => cursor.skip(2),
                _ => cursor.skip(1),
            }
        }
    }
}*/

/// The state of the tokenizer when it is in the middle of a raw double quoted string.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RawDoubleQuotedStringTailState {
    start_hash_count: usize,
}

impl RawDoubleQuotedStringTailState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, FullToken) {
        let mut s = String::new();
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
                        break (State::Initial(InitialState), FullToken::String(Rc::new(s)));
                    }
                }
                '\0' => {
                    break (State::RawDoubleQuotedStringTail(self), FullToken::String(Rc::new(s)));
                }
                x => {
                    s.push(x);
                    cursor.skip(1);
                }
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
    /// use makepad_live_tokenizer::tokenizer::Cursor;
    /// 
    /// let mut scratch = String::new();
    /// let cursor = Cursor::new(&['1', '2', '3'], &mut scratch);
    /// ```
    pub fn new(chars: &'a [char], scratch: &'a mut String) -> Cursor<'a> {
        Cursor {chars, scratch, index: 0 }
    }
    
    pub fn index(&self) -> usize {
        self.index
    }
    
    fn from_start_to_scratch(&mut self, start: usize) -> &str {
        self.scratch.clear();
        for i in start..self.index {
            self.scratch.push(self.chars[i]);
        }
        self.scratch
    }
    
    
    fn peek(&self, index: usize) -> char {
        self.chars.get(self.index + index).cloned().unwrap_or('\0')
    }
    
    fn id_from_1(&self) -> LiveId {
        LiveId::from_bytes(LIVE_ID_SEED, &[
            self.chars[self.index] as u8,
        ], 0, 1)
    }
    
    fn id_from_2(&self) -> LiveId {
        LiveId::from_bytes(LIVE_ID_SEED, &[
            self.chars[self.index] as u8,
            self.chars[self.index + 1] as u8,
        ], 0, 2)
    }
    
    fn id_from_3(&self) -> LiveId {
        LiveId::from_bytes(LIVE_ID_SEED, &[
            self.chars[self.index] as u8,
            self.chars[self.index + 1] as u8,
            self.chars[self.index + 2] as u8,
        ], 0, 3)
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
