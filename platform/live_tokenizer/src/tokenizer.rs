use {
    crate::{
        char_ext::CharExt,
        live_id::LiveId,
        full_token::{TokenWithLen, Delim, FullToken},
        colorhex
    },
};

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
            ('#', ch1, ch2) if ch1 == 'x' && ch2.is_hex() || ch1.is_hex() => self.color(cursor),
            ('.', ch1, _) if ch1.is_digit(10) => self.number(cursor),
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
                if "alse".chars().all( | expected | cursor.skip_if( | actual | actual == expected)) {
                    if !cursor.peek(0).is_identifier_continue() {
                        return (State::Initial(InitialState), FullToken::Bool(false));
                    }
                }
                self.identifier_tail(start, cursor)
            }
            't' => {
                cursor.skip(1);
                if "rue".chars().all( | expected | cursor.skip_if( | actual | actual == expected)) {
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
            LiveId::from_str(cursor.from_start_to_scratch(start)).unwrap()
        ))
    }
    
    fn number(self, cursor: &mut Cursor) -> (State, FullToken) {
        //debug_assert!(cursor.peek(0).is_digit(10));
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
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
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
                ('\\', '\'') | ('\\', '\\') => cursor.skip(2),
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
                ('\\', '"') => cursor.skip(2),
                _ => cursor.skip(1),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RawDoubleQuotedStringTailState {
    start_hash_count: usize,
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

#[derive(Debug)]
pub struct Cursor<'a> {
    chars: &'a [char],
    scratch: &'a mut String,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(chars: &'a [char], scratch: &'a mut String) -> Cursor<'a> {
        Cursor {chars, scratch, index: 0}
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
        LiveId::from_bytes(&[
            self.chars[self.index + 0] as u8,
        ], 0, 1)
    }
    
    fn id_from_2(&self) -> LiveId {
        LiveId::from_bytes(&[
            self.chars[self.index + 0] as u8,
            self.chars[self.index + 1] as u8,
        ], 0, 2)
    }
    
    fn id_from_3(&self) -> LiveId {
        LiveId::from_bytes(&[
            self.chars[self.index + 0] as u8,
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

