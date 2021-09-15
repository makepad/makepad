use {
    crate::{
        delta::{Delta, OperationRange},
        text::Text,
        token::Token,
        tokenizer::{Cursor, State},
    },
    std::{iter, slice::Iter},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TokenCache {
    lines: Vec<Option<Line>>,
}

impl TokenCache {
    pub fn new(text: &Text) -> TokenCache {
        let mut cache = TokenCache {
            lines: (0..text.as_lines().len()).map(|_| None).collect::<Vec<_>>(),
        };
        cache.refresh(text);
        cache
    }

    pub fn tokens_by_line(&self) -> TokensByLine {
        TokensByLine {
            iter: self.lines.iter(),
        }
    }

    pub fn invalidate(&mut self, delta: &Delta) {
        for operation_range in delta.operation_ranges() {
            match operation_range {
                OperationRange::Insert(range) => {
                    self.lines[range.start.line] = None;
                    self.lines.splice(
                        range.start.line + 1..range.start.line + 1,
                        iter::repeat(None).take(range.end.line - range.start.line),
                    );
                }
                OperationRange::Delete(range) => {
                    self.lines.drain(range.start.line..range.end.line);
                    self.lines[range.start.line] = None;
                }
            }
        }
    }

    pub fn refresh(&mut self, text: &Text) {
        let mut state = State::default();
        for (index, line) in self.lines.iter_mut().enumerate() {
            match line {
                Some(Line {
                    start_state,
                    end_state,
                    ..
                }) if state == *start_state => {
                    state = *end_state;
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
                    *line = Some(Line {
                        start_state,
                        tokens,
                        end_state: state,
                    });
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct TokensByLine<'a> {
    iter: Iter<'a, Option<Line>>,
}

impl<'a> Iterator for TokensByLine<'a> {
    type Item = &'a [Token];

    fn next(&mut self) -> Option<&'a [Token]> {
        Some(&self.iter.next()?.as_ref().unwrap().tokens)
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
struct Line {
    start_state: State,
    tokens: Vec<Token>,
    end_state: State,
}
