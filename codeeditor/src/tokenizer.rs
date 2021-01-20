#[derive(Clone, Copy, Debug)]
pub enum State {
    Initial,
    BlockCommentTail(usize),
    DoubleQuotedStringTail,
    RawDoubleQuotedStringTail(usize),
}

#[derive(Debug)]
pub struct Tokenize<'a> {
    state: &'a mut State,
    chars: &'a [char],
    index: usize,
}

impl<'a> Tokenize<'a> {
    fn line_comment(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == '/' && self.peek(1) == '/');
        self.skip(2);
        while self.accept(|ch| ch != '\0') {}
        TokenKind::Comment
    }

    fn block_comment(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == '/' && self.peek(1) == '*');
        self.skip(2);
        self.block_comment_tail(0)
    }

    fn block_comment_tail(&mut self, mut depth: usize) -> TokenKind {
        loop {
            match (self.peek(0), self.peek(1)) {
                ('/', '*') => {
                    self.skip(2);
                    depth += 1;
                }
                ('*', '/') => {
                    self.skip(2);
                    if depth == 0 {
                        *self.state = State::Initial;
                        break;
                    }
                    depth -= 1;
                }
                ('\0', _) => {
                    *self.state = State::BlockCommentTail(depth);
                    break;
                }
                _ => self.skip(1),
            }
        }
        TokenKind::Comment
    }

    fn char_or_lifetime(&mut self) -> TokenKind {
        if self.peek(1).is_identifier_start() && self.peek(2) != '\'' {
            debug_assert!(self.peek(0) == '\'');
            self.skip(2);
            while self.accept(|ch| ch.is_identifier_continue()) {}
            if self.peek(0) == '\'' {
                self.skip(1);
                self.suffix();
                TokenKind::String
            } else {
                TokenKind::Identifier
            }
        } else {
            self.single_quoted_string()
        }
    }

    fn string(&mut self) -> TokenKind {
        self.double_quoted_string()
    }

    fn byte(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == 'b');
        self.skip(1);
        self.single_quoted_string()
    }

    fn byte_string(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == 'b');
        self.skip(1);
        self.double_quoted_string()
    }

    fn raw_string(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == 'r');
        self.skip(1);
        self.raw_double_quoted_string()
    }

    fn raw_byte_string(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == 'b' && self.peek(1) == 'r');
        self.skip(2);
        self.raw_double_quoted_string()
    }

    fn single_quoted_string(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == '\'');
        self.skip(1);
        loop {
            match (self.peek(0), self.peek(1)) {
                ('\'', _) => {
                    self.skip(1);
                    self.suffix();
                    break;
                }
                ('\0', _) => return TokenKind::Unknown,
                ('\\', '\'') | ('\\', '\\') => self.skip(2),
                _ => self.skip(1),
            }
        }
        TokenKind::String
    }

    fn double_quoted_string(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == '"');
        self.skip(1);
        self.double_quoted_string_tail()
    }

    fn double_quoted_string_tail(&mut self) -> TokenKind {
        loop {
            match (self.peek(0), self.peek(1)) {
                ('"', _) => {
                    self.skip(1);
                    self.suffix();
                    *self.state = State::Initial;
                    break;
                }
                ('\0', _) => {
                    *self.state = State::DoubleQuotedStringTail;
                    break;
                }
                ('\\', '"') => self.skip(2),
                _ => self.skip(1),
            }
        }
        TokenKind::String
    }

    fn raw_double_quoted_string(&mut self) -> TokenKind {
        let mut start_hash_count = 0;
        while self.accept(|ch| ch == '#') {
            start_hash_count += 1;
        }
        self.raw_double_quoted_string_tail(start_hash_count)
    }

    fn raw_double_quoted_string_tail(&mut self, start_hash_count: usize) -> TokenKind {
        loop {
            match self.peek(0) {
                '"' => {
                    self.skip(1);
                    let mut end_hash_count = 0;
                    while end_hash_count < start_hash_count && self.accept(|ch| ch == '#') {
                        end_hash_count += 1;
                    }
                    if end_hash_count == start_hash_count {
                        self.suffix();
                        *self.state = State::Initial;
                        break;
                    }
                }
                '\0' => {
                    *self.state = State::RawDoubleQuotedStringTail(start_hash_count);
                    break;
                }
                _ => self.skip(1),
            }
        }
        TokenKind::String
    }

    fn raw_identifier(&mut self) -> TokenKind {
        debug_assert!(self.peek(0) == 'r' && self.peek(1).is_identifier_start());
        self.skip(3);
        self.identifier_tail()
    }

    fn identifier_or_keyword(&mut self) -> TokenKind {
        debug_assert!(self.peek(0).is_identifier_start());
        match self.peek(0) {
            'a' => {
                self.skip(1);
                match self.peek(0) {
                    'b' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("stract")
                    }
                    's' => {
                        self.skip(1);
                        match self.peek(0) {
                            'y' => self.identifier_or_keyword_tail("nc"),
                            _ => self.identifier_or_keyword_tail(""),
                        }
                    }
                    'w' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("ait")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'b' => {
                self.skip(1);
                match self.peek(0) {
                    'e' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("come")
                    }
                    'o' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("x")
                    }
                    'r' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("reak")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'c' => {
                self.skip(1);
                match self.peek(0) {
                    'o' => {
                        self.skip(1);
                        match self.peek(0) {
                            'n' => {
                                self.skip(1);
                                match self.peek(0) {
                                    's' => {
                                        self.skip(1);
                                        self.identifier_or_keyword_tail("t")
                                    }
                                    't' => {
                                        self.skip(1);
                                        self.identifier_or_keyword_tail("inue")
                                    }
                                    _ => self.identifier_tail(),
                                }
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    'r' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("ate")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'd' => {
                self.skip(1);
                match self.peek(0) {
                    'o' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("")
                    }
                    'y' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("n")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'e' => {
                self.skip(1);
                match self.peek(0) {
                    'l' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("se")
                    }
                    'n' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("um")
                    }
                    'x' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("tern")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'f' => {
                self.skip(1);
                match self.peek(0) {
                    'a' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("lse")
                    }
                    'i' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("nal")
                    }
                    'n' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("")
                    }
                    'o' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("r")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'i' => {
                self.skip(1);
                match self.peek(0) {
                    'f' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("")
                    }
                    'm' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("pl")
                    }
                    'n' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'l' => {
                self.skip(1);
                match self.peek(0) {
                    'e' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("t")
                    }
                    'o' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("op")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'm' => {
                self.skip(1);
                match self.peek(0) {
                    'a' => {
                        self.skip(1);
                        match self.peek(0) {
                            'c' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("ro")
                            }
                            't' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("ch")
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    'o' => {
                        self.skip(1);
                        match self.peek(0) {
                            'd' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("")
                            }
                            'v' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("e")
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    'u' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("t")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'o' => {
                self.skip(1);
                self.identifier_or_keyword_tail("verride")
            }
            'p' => {
                self.skip(1);
                match self.peek(0) {
                    'r' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("iv")
                    }
                    'u' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("b")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'r' => {
                self.skip(1);
                match self.peek(0) {
                    'e' => {
                        self.skip(1);
                        match self.peek(0) {
                            'f' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("")
                            }
                            't' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("urn")
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    _ => self.identifier_tail(),
                }
            }
            's' => {
                self.skip(1);
                match self.peek(0) {
                    'e' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("lf")
                    }
                    't' => {
                        self.skip(1);
                        match self.peek(0) {
                            'a' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("tic")
                            }
                            'r' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("uct")
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    'u' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("per")
                    }
                    _ => self.identifier_tail(),
                }
            }
            't' => {
                self.skip(1);
                match self.peek(0) {
                    'r' => {
                        self.skip(1);
                        match self.peek(0) {
                            'a' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("it")
                            }
                            'u' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("e")
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    'y' => {
                        self.skip(1);
                        match self.peek(0) {
                            'p' => {
                                self.skip(1);
                                match self.peek(0) {
                                    'e' => {
                                        self.skip(1);
                                        match self.peek(0) {
                                            'o' => {
                                                self.skip(1);
                                                self.identifier_or_keyword_tail("f")
                                            }
                                            _ => self.identifier_or_keyword_tail(""),
                                        }
                                    }
                                    _ => self.identifier_tail(),
                                }
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    _ => self.identifier_tail(),
                }
            }
            'u' => {
                self.skip(1);
                match self.peek(0) {
                    'n' => {
                        self.skip(1);
                        match self.peek(0) {
                            's' => {
                                self.skip(1);
                                match self.peek(0) {
                                    'a' => {
                                        self.skip(1);
                                        self.identifier_or_keyword_tail("fe")
                                    }
                                    'i' => {
                                        self.skip(1);
                                        self.identifier_or_keyword_tail("zed")
                                    }
                                    _ => self.identifier_tail(),
                                }
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    's' => {
                        self.skip(1);
                        self.identifier_or_keyword_tail("e")
                    }
                    _ => self.identifier_tail(),
                }
            }
            'v' => {
                self.skip(1);
                self.identifier_or_keyword_tail("irtual")
            }
            'w' => {
                self.skip(1);
                match self.peek(0) {
                    'h' => {
                        self.skip(1);
                        match self.peek(0) {
                            'e' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("re")
                            }
                            'i' => {
                                self.skip(1);
                                self.identifier_or_keyword_tail("le")
                            }
                            _ => self.identifier_tail(),
                        }
                    }
                    _ => self.identifier_tail(),
                }
            }
            'y' => {
                self.skip(1);
                self.identifier_or_keyword_tail("ield")
            }
            _ => self.identifier_tail(),
        }
    }

    fn identifier_or_keyword_tail(&mut self, string: &str) -> TokenKind {
        for expected in string.chars() {
            if !self.accept(|actual| actual == expected) {
                return TokenKind::Identifier;
            }
        }
        if self.peek(0).is_identifier_continue() {
            self.skip(1);
            return self.identifier_tail();
        }
        TokenKind::Keyword
    }

    fn identifier_tail(&mut self) -> TokenKind {
        while self.accept(|ch| ch.is_identifier_continue()) {}
        TokenKind::Identifier
    }

    fn number(&mut self) -> TokenKind {
        debug_assert!(self.peek(0).is_digit(10));
        match (self.peek(0), self.peek(1)) {
            ('0', 'b') => {
                self.skip(2);
                if !self.digits(2) {
                    return TokenKind::Unknown;
                }
            }
            ('0', 'o') => {
                self.skip(2);
                if !self.digits(8) {
                    return TokenKind::Unknown;
                }
            }
            ('0', 'x') => {
                self.skip(2);
                if !self.digits(16) {
                    return TokenKind::Unknown;
                }
            }
            _ => {
                self.digits(10);
                match self.peek(0) {
                    '.' if self.peek(1) != '.' && !self.peek(0).is_identifier_start() => {
                        if self.digits(10) {
                            if self.peek(0) == 'E' || self.peek(1) == 'e' {
                                if !self.exponent() {
                                    return TokenKind::Unknown;
                                }
                            }
                        }
                    }
                    'E' | 'e' => {
                        if !self.exponent() {
                            return TokenKind::Unknown;
                        }
                    }
                    _ => {}
                }
            }
        };
        self.suffix();
        TokenKind::Number
    }

    fn exponent(&mut self) -> bool {
        debug_assert!(self.peek(0) == 'E' || self.peek(0) == 'e');
        self.skip(1);
        if self.peek(0) == '+' || self.peek(1) == '-' {
            self.skip(1);
        }
        self.digits(10)
    }

    fn digits(&mut self, radix: u32) -> bool {
        let mut has_digits = false;
        loop {
            match self.peek(0) {
                '_' => {
                    self.skip(1);
                }
                ch if ch.is_digit(radix) => {
                    self.skip(1);
                    has_digits = true;
                }
                _ => break,
            }
        }
        has_digits
    }

    fn suffix(&mut self) {
        if self.peek(0).is_identifier_start() {
            self.skip(1);
            while self.accept(|ch| ch.is_identifier_continue()) {}
        }
    }

    fn whitespace(&mut self) -> TokenKind {
        debug_assert!(self.peek(0).is_whitespace());
        self.skip(1);
        while self.accept(|ch| ch.is_whitespace()) {}
        TokenKind::Whitespace
    }

    fn accept<P>(&mut self, predicate: P) -> bool
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

    fn peek(&self, index: usize) -> char {
        self.chars.get(self.index + index).cloned().unwrap_or('\0')
    }

    fn skip(&mut self, count: usize) {
        self.index += count;
    }
}

impl<'a> Iterator for Tokenize<'a> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        let start = self.index;
        let kind = match *self.state {
            State::Initial => match (self.peek(0), self.peek(1), self.peek(2)) {
                ('/', '/', _) => self.line_comment(),
                ('/', '*', _) => self.block_comment(),
                ('.', '.', '.') | ('.', '.', '=') | ('<', '<', '=') | ('>', '>', '=') => {
                    self.skip(3);
                    TokenKind::Punctuator
                }
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
                    self.skip(2);
                    TokenKind::Punctuator
                }
                ('!', _, _)
                | ('#', _, _)
                | ('$', _, _)
                | ('%', _, _)
                | ('&', _, _)
                | ('(', _, _)
                | (')', _, _)
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
                | ('{', _, _)
                | ('|', _, _)
                | ('}', _, _) => {
                    self.skip(1);
                    TokenKind::Punctuator
                }
                ('\'', _, _) => self.char_or_lifetime(),
                ('"', _, _) => self.string(),
                ('b', '\'', _) => self.byte(),
                ('b', '"', _) => self.byte_string(),
                ('b', 'r', '"') | ('b', 'r', '#') => self.raw_byte_string(),
                ('r', '#', '"') | ('r', '#', '#') => self.raw_string(),
                ('r', ch, _) if ch.is_identifier_start() => self.raw_identifier(),
                ('\0', _, _) => return None,
                (ch, _, _) if ch.is_identifier_start() => self.identifier_or_keyword(),
                (ch, _, _) if ch.is_digit(10) => self.number(),
                (ch, _, _) if ch.is_whitespace() => self.whitespace(),
                _ => {
                    self.skip(1);
                    TokenKind::Unknown
                }
            },
            State::BlockCommentTail(depth) => self.block_comment_tail(depth),
            State::DoubleQuotedStringTail => self.double_quoted_string_tail(),
            State::RawDoubleQuotedStringTail(start_hash_count) => {
                self.raw_double_quoted_string_tail(start_hash_count)
            }
        };
        assert!(start != self.index);
        Some(Token {
            len: self.index - start,
            kind,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug)]
pub enum TokenKind {
    Comment,
    Identifier,
    Punctuator,
    Keyword,
    Number,
    String,
    Whitespace,
    Unknown,
}

trait CharExt {
    fn is_identifier_start(self) -> bool;

    fn is_identifier_continue(self) -> bool;
}

impl CharExt for char {
    fn is_identifier_start(self) -> bool {
        match self {
            'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }

    fn is_identifier_continue(self) -> bool {
        match self {
            '0'..='9' | 'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }
}

pub fn tokenize<'a>(state: &'a mut State, chars: &'a [char]) -> Tokenize<'a> {
    Tokenize {
        state,
        chars,
        index: 0,
    }
}
