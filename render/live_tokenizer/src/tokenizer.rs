use {
    crate::{
        char::CharExt,
        live_id::LiveId,
        token::{TokenWithLen, Delim, TokenKind},
    }
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
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<TokenWithLen>) {
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
            Some(TokenWithLen {
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
                let id = cursor.id_from_3();
                cursor.skip(3);
                (
                    State::Initial(InitialState),
                    TokenKind::Punct(id),
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
                    TokenKind::Punct(id),
                )
            }
            ('\'', _, _) => self.char_or_lifetime(cursor),
            ('"', _, _) => self.string(cursor),
            ('(', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Open(Delim::Paren),
                )
            }
            (')', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Close(Delim::Paren),
                )
            }
            ('[', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Open(Delim::Bracket),
                )
            }
            (']', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Close(Delim::Bracket),
                )
            }
            ('{', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Open(Delim::Brace),
                )
            }
            ('}', _, _) => {
                cursor.skip(1);
                (
                    State::Initial(InitialState),
                    TokenKind::Close(Delim::Brace),
                )
            }
            ('#', ch1, ch2) if ch1 == 'x' && ch2.is_hex() || ch1.is_hex() => self.color(cursor),
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
                    TokenKind::Punct(id),
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
        while cursor.skip_if( | ch | ch != '\0') {}
        (State::Initial(InitialState), TokenKind::Comment)
    }
    
    fn block_comment(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == '/' && cursor.peek(1) == '*');
        cursor.skip(2);
        BlockCommentTailState {depth: 0}.next(cursor)
    }
    
    fn identifier_or_keyword(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        let start = cursor.index();
        match cursor.peek(0) {
            'a' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'b' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("stract",  start, cursor)
                    }
                    's' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'y' => self.identifier_or_keyword_tail("nc", start, cursor),
                            _ => self.identifier_or_keyword_tail("",  start, cursor),
                        }
                    }
                    'w' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("ait", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'b' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'e' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("come", start, cursor)
                    }
                    'o' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("x", start, cursor)
                    }
                    'r' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("reak",  start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
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
                                        self.identifier_or_keyword_tail("t",  start, cursor)
                                    }
                                    't' => {
                                        cursor.skip(1);
                                        self.identifier_or_keyword_tail(
                                            "inue",
                                            start,
                                            cursor,
                                        )
                                    }
                                    _ => self.identifier_tail(start, cursor),
                                }
                            }
                            _ => self.identifier_tail(start, cursor),
                        }
                    }
                    'r' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("ate",  start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'd' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'o' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("", start, cursor)
                    }
                    'y' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("n", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'e' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'l' => {
                        cursor.skip(1);
                        self.identifier_or_branch_tail("se", start, cursor)
                    }
                    'n' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("um", start, cursor)
                    }
                    'x' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("tern", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'f' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'a' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("lse", start, cursor)
                    }
                    'i' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("nal", start, cursor)
                    }
                    'n' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("", start, cursor)
                    }
                    'o' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("r", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'i' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'f' => {
                        cursor.skip(1);
                        self.identifier_or_branch_tail("", start, cursor)
                    }
                    'm' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("pl", start, cursor)
                    }
                    'n' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'l' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'e' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("t", start, cursor)
                    }
                    'o' => {
                        cursor.skip(1);
                        self.identifier_or_loop_tail("op", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
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
                                self.identifier_or_keyword_tail("ro", start, cursor)
                            }
                            't' => {
                                cursor.skip(1);
                                self.identifier_or_branch_tail("ch", start, cursor)
                            }
                            _ => self.identifier_tail(start, cursor),
                        }
                    }
                    'o' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'd' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("", start, cursor)
                            }
                            'v' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("e",start, cursor)
                            }
                            _ => self.identifier_tail(start, cursor),
                        }
                    }
                    'u' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("t", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'o' => {
                cursor.skip(1);
                self.identifier_or_keyword_tail("verride", start, cursor)
            }
            'p' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'r' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("iv", start, cursor)
                    }
                    'u' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("b", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
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
                                self.identifier_or_keyword_tail("", start, cursor)
                            }
                            't' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("urn", start, cursor)
                            }
                            _ => self.identifier_tail(start, cursor),
                        }
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            's' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'e' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("lf", start, cursor)
                    }
                    't' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'a' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("tic", start, cursor)
                            }
                            'r' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("uct", start, cursor)
                            }
                            _ => self.identifier_tail(start, cursor),
                        }
                    }
                    'u' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("per", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
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
                                self.identifier_or_keyword_tail("it", start, cursor)
                            }
                            'u' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("e",  start, cursor)
                            }
                            _ => self.identifier_tail(start, cursor),
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
                                                    start,
                                                    cursor,
                                                )
                                            }
                                            _ => self.identifier_or_keyword_tail(
                                                "",
                                                start,
                                                cursor,
                                            ),
                                        }
                                    }
                                    _ => self.identifier_tail(start, cursor),
                                }
                            }
                            _ => self.identifier_tail(start, cursor),
                        }
                    }
                    _ => self.identifier_tail(start, cursor),
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
                                            start,
                                            cursor,
                                        )
                                    }
                                    'i' => {
                                        cursor.skip(1);
                                        self.identifier_or_keyword_tail(
                                            "zed",
                                            start,
                                            cursor,
                                        )
                                    }
                                    _ => self.identifier_tail(start, cursor),
                                }
                            }
                            _ => self.identifier_tail(start, cursor),
                        }
                    }
                    's' => {
                        cursor.skip(1);
                        self.identifier_or_keyword_tail("e", start, cursor)
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'v' => {
                cursor.skip(1);
                self.identifier_or_keyword_tail("irtual", start, cursor)
            }
            'w' => {
                cursor.skip(1);
                match cursor.peek(0) {
                    'h' => {
                        cursor.skip(1);
                        match cursor.peek(0) {
                            'e' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("re", start, cursor)
                            }
                            'i' => {
                                cursor.skip(1);
                                self.identifier_or_keyword_tail("le", start, cursor)
                            }
                            _ => self.identifier_tail(start, cursor),
                        }
                    }
                    _ => self.identifier_tail(start, cursor),
                }
            }
            'y' => {
                cursor.skip(1);
                self.identifier_or_keyword_tail("ield", start, cursor)
            }
            _ => self.identifier_tail(start, cursor),
        }
    }
    
    fn identifier_or_keyword_tail(self, string: &str, start: usize, cursor: &mut Cursor,) -> (State, TokenKind) {
        if string.chars().all( | expected | cursor.skip_if( | actual | actual == expected)){
            if !cursor.peek(0).is_identifier_continue() {
                return (State::Initial(InitialState), TokenKind::Keyword(LiveId::from_char_slice(cursor.slice_from_start(start))));
            }
        }
        self.identifier_tail(start, cursor)
    }

    fn identifier_or_branch_tail(self, string: &str, start: usize, cursor: &mut Cursor,) -> (State, TokenKind) {
        if string.chars().all( | expected | cursor.skip_if( | actual | actual == expected)){
            if !cursor.peek(0).is_identifier_continue() {
                return (State::Initial(InitialState), TokenKind::Branch(LiveId::from_char_slice(cursor.slice_from_start(start))));
            }
        }
        self.identifier_tail(start, cursor)
    }

    fn identifier_or_loop_tail(self, string: &str, start: usize, cursor: &mut Cursor,) -> (State, TokenKind) {
        if string.chars().all( | expected | cursor.skip_if( | actual | actual == expected)){
            if !cursor.peek(0).is_identifier_continue() {
                return (State::Initial(InitialState), TokenKind::Loop(LiveId::from_char_slice(cursor.slice_from_start(start))));
            }
        }
        self.identifier_tail(start, cursor)
    }

    
    fn identifier_tail(self, start: usize, cursor: &mut Cursor) -> (State, TokenKind) {
        while cursor.skip_if( | ch | ch.is_identifier_continue()) {}
        (State::Initial(InitialState), TokenKind::Ident(LiveId::from_char_slice(cursor.slice_from_start(start))))
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
    
    fn color(self, cursor: &mut Cursor) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('#', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
            }
            _ => {
                cursor.skip(1);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
            }
        };
        (State::Initial(InitialState), TokenKind::Color)
    }
    
    fn char_or_lifetime(self, cursor: &mut Cursor) -> (State, TokenKind) {
        if cursor.peek(1).is_identifier_start() && cursor.peek(2) != '\'' {
            debug_assert!(cursor.peek(0) == '\'');
            cursor.skip(2);
            while cursor.skip_if( | ch | ch.is_identifier_continue()) {}
            if cursor.peek(0) == '\'' {
                cursor.skip(1);
                cursor.skip_suffix();
                (State::Initial(InitialState), TokenKind::String)
            } else {
                (State::Initial(InitialState), TokenKind::Lifetime)
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
        while cursor.skip_if( | ch | ch == '#') {
            start_hash_count += 1;
        }
        RawDoubleQuotedStringTailState {start_hash_count}.next(cursor)
    }
    
    fn whitespace(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if( | ch | ch.is_whitespace()) {}
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
                    while end_hash_count < self.start_hash_count && cursor.skip_if( | ch | ch == '#') {
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
        Cursor {chars, index: 0}
    }
    
    fn index(&self) -> usize {
        self.index
    }
    
    
    fn slice_from_start(&self, start: usize) -> &[char] {
        &self.chars[start..self.index]
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
            while self.skip_if( | ch | ch.is_identifier_continue()) {}
        }
    }
}
