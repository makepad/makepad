use {
    crate::{
        code_editor::{
            delta::{Delta, OperationRange},
            text::Text,
            token::Token,
            tokenizer::{Cursor, State},
        }
    },
    std::{iter, ops::{Deref, Index}, slice::Iter},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TokenCache {
    lines: Vec<Line>,
}

impl TokenCache {
    pub fn new(text: &Text) -> TokenCache {
        let mut cache = TokenCache {
            lines: (0..text.as_lines().len()).map(|_| Line::default()).collect::<Vec<_>>(),
        };
        cache.refresh(text);
        cache
    }

    pub fn invalidate(&mut self, delta: &Delta) {
        for operation_range in delta.operation_ranges() {
            match operation_range {
                OperationRange::Insert(range) => {
                    self.lines[range.start.line] = Line::default();
                    self.lines.splice(
                        range.start.line..range.start.line,
                        iter::repeat(Line::default()).take(range.end.line - range.start.line),
                    );
                }
                OperationRange::Delete(range) => {
                    self.lines.drain(range.start.line..range.end.line);
                    self.lines[range.start.line] = Line::default();
                }
            }
        }
    }

    pub fn refresh(&mut self, text: &Text) {
        let mut state = State::default();
        for (index, line) in self.lines.iter_mut().enumerate() {
            match line.token_info {
                Some(TokenInfo {
                    start_state,
                    end_state,
                    ..
                }) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut tokens = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[index]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => tokens.push(token),
                            None => break,
                        }
                    }
                    line.token_info = Some(TokenInfo {
                        start_state,
                        tokens,
                        end_state: state,
                    });
                }
            }
        }
    }
}

impl Deref for TokenCache {
    type Target = [Line];

    fn deref(&self) -> &Self::Target {
        &self.lines
    }
}

impl Index<usize> for TokenCache {
    type Output = Line;

    fn index(&self, index: usize) -> &Self::Output {
        &self.lines[index]
    }
}

impl<'a> IntoIterator for &'a TokenCache {
    type Item = &'a Line;
    type IntoIter = Iter<'a, Line>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Line {
    token_info: Option<TokenInfo>
}

impl Line {
    pub fn tokens(&self) -> &[Token] {
        &self.token_info.as_ref().unwrap().tokens
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
struct TokenInfo {
    start_state: State,
    tokens: Vec<Token>,
    end_state: State,
}
