pub enum State {
    Initial(InitialState),
    BlockCommentTail(BlockCommentTailState),
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
            State::BlockCommentTail(state) => state.next(cursor)
        };
        let end = cursor.index;
        assert!(start < end);
        (
            next_state,
            Some(Token {
                len: end - start,
                kind,
            })
        )
    }
}

pub struct InitialState;

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('/', '*', _) => self.block_comment(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Unknown)
            }
        }
    }

    fn block_comment(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0) == '/' && cursor.peek(1) == '*');
        cursor.skip(2);
        BlockCommentTailState { depth: 0 }.next(cursor)
    }
}

pub struct BlockCommentTailState { depth: usize }

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

pub struct Cursor<'a> {
    chars: &'a [char],
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(chars: &'a [char]) -> Cursor<'a> {
        Cursor {
            chars,
            index: 0
        }
    }

    fn peek(&self, index: usize) -> char {
        self.chars.get(self.index + index).cloned().unwrap_or('\0')
    }

    fn skip(&mut self, count: usize) {
        self.index += count;
    }
}

#[derive(Debug)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Debug)]
pub enum TokenKind {
    Comment,
    Unknown,
}
