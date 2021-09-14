use crate::{
    char::CharExt,
    token::{Keyword, Punctuator, Token, TokenKind},
};

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
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<Token>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, kind) = match self {
            State::Initial(state) => state.next(cursor),
            State::BlockCommentTail(state) => state.next(cursor),
            State::DoubleQuotedStringTail(state) => state.next(cursor),
            State::RawDoubleQuotedStringTail(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (
            next_state,
            Some(Token {
                len: end - start,
                kind,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InitialState;

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('r', '#', '"') | ('r', '#', '#') => self.raw_string(cursor),
            ('b', 'r', '"') | ('b', 'r', '#') => self.raw_byte_string(cursor),
            ('.', '.', '.') | ('.', '.', '=') | ('<', '<', '=') | ('>', '>', '=') => {
                cursor.skip(3);
                (
                    State::Initial(InitialState),
                    TokenKind::Punctuator(Punctuator::Other),
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
                cursor.skip(2);
                (
                    State::Initial(InitialState),
                    TokenKind::Punctuator(Punctuator::Other),
                )
            }
            ('\'', _, _) => self.char_or_lifetime(cursor),
            ('"', _, _) => self.string(cursor),
            ('(', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Punctuator(Punctuator::LeftParen),
                )
            }
            (')', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Punctuator(Punctuator::RightParen),
                )
            }
            ('{', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Punctuator(Punctuator::LeftBrace),
                )
            }
            ('}', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Punctuator(Punctuator::RightBrace),
                )
            }
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
            | ('[', _, _)
            | (']', _, _)
            | ('^', _, _)
            | ('_', _, _)
            | ('|', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Punctuator(Punctuator::Other),
                )
            }
            (ch, _, _) if ch.is_identifier_start() => self.identifier_or_keyword(cursor),
            (ch, _, _) if ch.is_digit(10) => self.number(cursor),
            (ch, _, _) if ch.is_whitespace() => self.whitespace(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Unknown)
            }
        }
    }

    fn line_comment(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == '/' && cursor.peek(1) == '/');
        cursor.skip(2);
        while cursor.skip_if(|ch| ch != '\0') {}
        (State::Initial(InitialState), TokenKind::Comment)
    }

    fn block_comment(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == '/' && cursor.peek(1) == '*');
        cursor.skip(2);
        BlockCommentTailState { depth: 0 }.next(cursor)
    }

    fn identifier_or_keyword(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        match cursor.peek(0) {
            'a' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'b' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("stract", Keyword::Other, cursor)
                    }
                    's' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'y' => self.identifier_or_keyword_tail("nc", Keyword::Other, cursor),
                            _ => self.identifier_or_keyword_tail("", Keyword::Other, cursor),
                        }
                    }
                    'w' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("ait", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'b' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'e' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("come", Keyword::Other, cursor)
                    }
                    'o' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("x", Keyword::Other, cursor)
                    }
                    'r' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("reak", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'c' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'o' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'n' => {
                                cursor.skip(1);
                                match cursor.peek(0) {
                                    's' => {
                                        cursor.skip(1);
                                        self.identifier_or_keyword_tail("t", Keyword::Other, cursor)
                                    }
                                    't' => {
                                        cursor.skip(1);
                                        self.identifier_or_keyword_tail(
                                            "inue",
                                            Keyword::Other,
                                            cursor,
                                        )
                                    }
                                    _ => self.identifier_tail(cursor),
                                }
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    'r' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("ate", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'd' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'o' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("", Keyword::Other, cursor)
                    }
                    'y' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("n", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'e' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'l' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("se", Keyword::Branch, cursor)
                    }
                    'n' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("um", Keyword::Other, cursor)
                    }
                    'x' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("tern", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'f' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'a' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("lse", Keyword::Other, cursor)
                    }
                    'i' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("nal", Keyword::Other, cursor)
                    }
                    'n' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("", Keyword::Other, cursor)
                    }
                    'o' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("r", Keyword::Loop, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'i' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'f' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("", Keyword::Branch, cursor)
                    }
                    'm' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("pl", Keyword::Other, cursor)
                    }
                    'n' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'l' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'e' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("t", Keyword::Other, cursor)
                    }
                    'o' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("op", Keyword::Loop, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'm' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'a' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'c' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("ro", Keyword::Other, cursor)
                            }
                            't' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("ch", Keyword::Branch, cursor)
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    'o' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'd' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("", Keyword::Other, cursor)
                            }
                            'v' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("e", Keyword::Other, cursor)
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    'u' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("t", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'o' => {
                cursor.skip(1);
                self.identifier_or_keyword_tail("verride", Keyword::Other, cursor)
            }
            'p' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'r' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("iv", Keyword::Other, cursor)
                    }
                    'u' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("b", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'r' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'e' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'f' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("", Keyword::Other, cursor)
                            }
                            't' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("urn", Keyword::Other, cursor)
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            's' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'e' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("lf", Keyword::Other, cursor)
                    }
                    't' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'a' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("tic", Keyword::Other, cursor)
                            }
                            'r' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("uct", Keyword::Other, cursor)
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    'u' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("per", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            't' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'r' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'a' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("it", Keyword::Other, cursor)
                            }
                            'u' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("e", Keyword::Other, cursor)
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    'y' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'p' => {
                                cursor.skip(1);
                                match cursor.peek(0) {
                                    'e' => {
                                        cursor.skip(1);
                                        match cursor.peek(0) {
                                            'o' => {
                                                cursor.skip(1);
                                                self.identifier_or_keyword_tail(
                                                    "f",
                                                    Keyword::Other,
                                                    cursor,
                                                )
                                            }
                                            _ => self.identifier_or_keyword_tail(
                                                "",
                                                Keyword::Other,
                                                cursor,
                                            ),
                                        }
                                    }
                                    _ => self.identifier_tail(cursor),
                                }
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'u' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'n' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            's' => {
                                cursor.skip(1);
                                match cursor.peek(0) {
                                    'a' => {
                                        cursor.skip(1);
                                        self.identifier_or_keyword_tail(
                                            "fe",
                                            Keyword::Other,
                                            cursor,
                                        )
                                    }
                                    'i' => {
                                        cursor.skip(1);
                                        self.identifier_or_keyword_tail(
                                            "zed",
                                            Keyword::Other,
                                            cursor,
                                        )
                                    }
                                    _ => self.identifier_tail(cursor),
                                }
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    's' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("e", Keyword::Other, cursor)
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'v' => {
                cursor.skip(1);
                self.identifier_or_keyword_tail("irtual", Keyword::Other, cursor)
            }
            'w' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'h' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'e' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("re", Keyword::Other, cursor)
                            }
                            'i' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("le", Keyword::Loop, cursor)
                            }
                            _ => self.identifier_tail(cursor),
                        }
                    }
                    _ => self.identifier_tail(cursor),
                }
            }
            'y' => {
                cursor.skip(1);
                self.identifier_or_keyword_tail("ield", Keyword::Other, cursor)
            }
            _ => self.identifier_tail(cursor),
        }
    }

    fn identifier_or_keyword_tail(
        self,
        string: &str,
        keyword: Keyword,
        cursor: &mut Cursor,
    ) -> (State, TokenKind) {
        for expected in string.chars() {
            if !cursor.skip_if(|actual| actual == expected) {
                return (State::Initial(InitialState), TokenKind::Identifier);
            }
        }
        if cursor.peek(0).is_identifier_continue() {
            cursor.skip(1);
            return self.identifier_tail(cursor);
        }
        (State::Initial(InitialState), TokenKind::Keyword(keyword))
    }

    fn identifier_tail(self, cursor: &mut Cursor) -> (State, TokenKind) {
        while cursor.skip_if(|ch| ch.is_identifier_continue()) {}
        (State::Initial(InitialState), TokenKind::Identifier)
    }

    fn number(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_digit(10));
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
            }
            _ => {
                cursor.skip_digits(10);
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
                        if cursor.skip_digits(10) {
                            if cursor.peek(0) == 'E' || cursor.peek(1) == 'e' {
                                if !cursor.skip_exponent() {
                                    return (State::Initial(InitialState), TokenKind::Unknown);
                                }
                            }
                        }
                    }
                    'E' | 'e' => {
                        if !cursor.skip_exponent() {
                            return (State::Initial(InitialState), TokenKind::Unknown);
                        }
                    }
                    _ => {}
                }
            }
        };
        cursor.skip_suffix();
        (State::Initial(InitialState), TokenKind::Number)
    }

    fn char_or_lifetime(self, cursor: &mut Cursor) -> (State, TokenKind) {
        if cursor.peek(1).is_identifier_start() && cursor.peek(2) != '\'' {
            debug_assert!(cursor.peek(0) == '\'');
            cursor.skip(2);
            while cursor.skip_if(|ch| ch.is_identifier_continue()) {}
            if cursor.peek(0) == '\'' {
                cursor.skip(1);
                cursor.skip_suffix();
                (State::Initial(InitialState), TokenKind::String)
            } else {
                (State::Initial(InitialState), TokenKind::Identifier)
            }
        } else {
            self.single_quoted_string(cursor)
        }
    }

    fn byte(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == 'b');
        cursor.skip(1);
        self.single_quoted_string(cursor)
    }

    fn string(self, cursor: &mut Cursor) -> (State, TokenKind) {
        self.double_quoted_string(cursor)
    }

    fn byte_string(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == 'b');
        cursor.skip(1);
        self.double_quoted_string(cursor)
    }

    fn raw_string(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == 'r');
        cursor.skip(1);
        self.raw_double_quoted_string(cursor)
    }

    fn raw_byte_string(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == 'b' && cursor.peek(1) == 'r');
        cursor.skip(2);
        self.raw_double_quoted_string(cursor)
    }

    fn single_quoted_string(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == '\'');
        cursor.skip(1);
        loop {
            match (cursor.peek(0), cursor.peek(1)) {
                ('\'', _) => {
                    cursor.skip(1);
                    cursor.skip_suffix();
                    break;
                }
                ('\0', _) => return (State::Initial(InitialState), TokenKind::Unknown),
                ('\\', '\'') | ('\\', '\\') => cursor.skip(2),
                _ => cursor.skip(1),
            }
        }
        (State::Initial(InitialState), TokenKind::String)
    }

    fn double_quoted_string(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == '"');
        cursor.skip(1);
        DoubleQuotedStringTailState.next(cursor)
    }

    fn raw_double_quoted_string(self, cursor: &mut Cursor) -> (State, TokenKind) {
        let mut start_hash_count = 0;
        while cursor.skip_if(|ch| ch == '#') {
            start_hash_count += 1;
        }
        RawDoubleQuotedStringTailState { start_hash_count }.next(cursor)
    }

    fn whitespace(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if(|ch| ch.is_whitespace()) {}
        (State::Initial(InitialState), TokenKind::Whitespace)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BlockCommentTailState {
    depth: usize,
}

impl BlockCommentTailState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
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
                        break (State::Initial(InitialState), TokenKind::Comment);
                    }
                    state.depth -= 1;
                }
                ('\0', _) => {
                    break (State::BlockCommentTail(state), TokenKind::Comment);
                }
                _ => cursor.skip(1),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DoubleQuotedStringTailState;

impl DoubleQuotedStringTailState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        loop {
            match (cursor.peek(0), cursor.peek(1)) {
                ('"', _) => {
                    cursor.skip(1);
                    cursor.skip_suffix();
                    break (State::Initial(InitialState), TokenKind::String);
                }
                ('\0', _) => {
                    break (
                        State::DoubleQuotedStringTail(DoubleQuotedStringTailState),
                        TokenKind::String,
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
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        loop {
            match cursor.peek(0) {
                '"' => {
                    cursor.skip(1);
                    let mut end_hash_count = 0;
                    while end_hash_count < self.start_hash_count && cursor.skip_if(|ch| ch == '#') {
                        end_hash_count += 1;
                    }
                    if end_hash_count == self.start_hash_count {
                        cursor.skip_suffix();
                        break (State::Initial(InitialState), TokenKind::String);
                    }
                }
                '\0' => {
                    break (State::RawDoubleQuotedStringTail(self), TokenKind::String);
                }
                _ => cursor.skip(1),
            }
        }
    }
}

#[derive(Debug)]
pub struct Cursor<'a> {
    chars: &'a [char],
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(chars: &'a [char]) -> Cursor<'a> {
        Cursor { chars, index: 0 }
    }

    fn peek(&self, index: usize) -> char {
        self.chars.get(self.index + index).cloned().unwrap_or('\0')
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
        if self.peek(0) == '+' || self.peek(1) == '-' {
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

    fn skip_suffix(&mut self) {
        if self.peek(0).is_identifier_start() {
            self.skip(1);
            while self.skip_if(|ch| ch.is_identifier_continue()) {}
        }
    }
}
