use crate::{
    token::{TokenInfo, TokenKind},
    Diff, Text,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
    token_infos: Vec<Vec<TokenInfo>>,
}

impl Tokenizer {
    pub fn new(text: &Text) -> Self {
        let line_count = text.as_lines().len();
        let mut tokenizer = Self {
            state: (0..line_count).map(|_| None).collect(),
            token_infos: (0..line_count).map(|_| Vec::new()).collect(),
        };
        tokenizer.retokenize(&Diff::new(), text);
        tokenizer
    }

    pub fn token_infos(&self) -> &[Vec<TokenInfo>] {
        &self.token_infos
    }

    pub fn retokenize(&mut self, diff: &Diff, text: &Text) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    self.state.drain(line..line + length.line_count);
                    self.token_infos.drain(line..line + length.line_count);
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                    self.state
                        .splice(line..line, (0..length.line_count).map(|_| None));
                    self.token_infos
                        .splice(line..line, (0..length.line_count).map(|_| Vec::new()));
                    line += length.line_count;
                }
            }
        }
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut token_infos = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => token_infos.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    self.token_infos[line] = token_infos;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Initial(InitialState),
}

impl Default for State {
    fn default() -> State {
        State::Initial(InitialState)
    }
}

impl State {
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<TokenInfo>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, token_kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (next_state, Some(TokenInfo::new(end - start, token_kind)))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InitialState;

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
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
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            ('.', char, _) if char.is_digit(10) => self.number(cursor),
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
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            (char, _, _) if char.is_identifier_start() => self.identifier_or_keyword(cursor),
            (char, _, _) if char.is_digit(10) => self.number(cursor),
            (char, _, _) if char.is_whitespace() => self.whitespace(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Unknown)
            }
        }
    }

    fn identifier_or_keyword(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        let start = cursor.index;
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_identifier_continue()) {}
        let end = cursor.index;

        (
            State::Initial(InitialState),
            match &cursor.string[start..end] {
                "Self" | "as" | "async" | "await" | "break" | "const" | "continue" | "crate"
                | "dyn" | "else" | "enum" | "extern" | "false" | "fn" | "for" | "if" | "impl"
                | "in" | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub" | "ref"
                | "return" | "self" | "static" | "struct" | "super" | "trait" | "true" | "type"
                | "unsafe" | "use" | "where" | "while" => TokenKind::Keyword,
                _ => TokenKind::Identifier,
            },
        )
    }

    fn number(self, cursor: &mut Cursor) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            _ => {
                cursor.skip_digits(10);
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
                        cursor.skip(1);
                        if cursor.skip_digits(10) {
                            if cursor.peek(0) == 'E' || cursor.peek(0) == 'e' {
                                if !cursor.skip_exponent() {
                                    return (State::Initial(InitialState), TokenKind::Unknown);
                                }
                            }
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    'E' | 'e' => {
                        if !cursor.skip_exponent() {
                            return (State::Initial(InitialState), TokenKind::Unknown);
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    _ => {
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                }
            }
        };
    }

    fn whitespace(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_whitespace()) {}
        (State::Initial(InitialState), TokenKind::Whitespace)
    }
}

#[derive(Debug)]
pub struct Cursor<'a> {
    string: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(string: &'a str) -> Self {
        Cursor { string, index: 0 }
    }

    fn peek(&self, index: usize) -> char {
        self.string[self.index..].chars().nth(index).unwrap_or('\0')
    }

    fn skip(&mut self, count: usize) {
        self.index = self.string[self.index..]
            .char_indices()
            .nth(count)
            .map_or(self.string.len(), |(index, _)| self.index + index);
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
                char if char.is_digit(radix) => {
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
            while self.skip_if(|char| char.is_identifier_continue()) {}
            return true;
        }
        false
    }
}

pub trait CharExt {
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
