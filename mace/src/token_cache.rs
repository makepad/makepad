use {
    crate::{
        delta::{Delta, OperationSpan},
        token::Token,
        tokenizer::{Cursor, InitialState, State},
        text::Text,
    },
    std::slice::Iter,
};

pub struct TokenCache {
    line_datas: Vec<Option<LineData>>
}

impl TokenCache {
    pub fn new(text: &Text) -> TokenCache {
        let mut token_cache = TokenCache {
            line_datas: (0..text.as_lines().len()).map(|_| None).collect::<Vec<_>>()
        };
        token_cache.refresh(text);
        token_cache
    }

    pub fn lines(&self) -> Lines {
        Lines {
            iter: self.line_datas.iter(),
        }
    }

    pub fn invalidate(&mut self, delta: &Delta) {
        let mut line = 0;
        for operation in delta {
            match operation.span() {
                OperationSpan::Retain(count) => {
                    line += count.line;
                }
                OperationSpan::Insert(count) => {
                    self.line_datas[line] = None;
                    self.line_datas
                        .splice(line + 1..line + 1, (0..count.line).map(|_| None));
                    line += count.line;
                    if count.column > 0 {
                        self.line_datas[line] = None;
                    }
                }
                OperationSpan::Delete(count) => {
                    self.line_datas[line] = None;
                    self.line_datas
                        .drain(line + 1..line + 1 + count.line);
                    if count.column > 0 {
                        self.line_datas[line] = None;
                    }
                }
            }
        }
    }

    pub fn refresh(&mut self, text: &Text) {
        let mut state = State::Initial(InitialState);
        for (line, line_data) in self.line_datas.iter_mut().enumerate() {
            match line_data {
                Some(LineData {
                    start_state,
                    end_state,
                    ..
                }) if state == *start_state => {
                    state = *end_state;
                }
                _ => {
                    let start_state = state;
                    let mut tokens = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => tokens.push(token),
                            None => break,
                        }
                    }
                    *line_data = Some(LineData {
                        start_state,
                        end_state: state,
                        tokens,
                    });
                }
            }
        }
    }
}

pub struct Lines<'a> {
    iter: Iter<'a, Option<LineData>>
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a Vec<Token>;

    fn next(&mut self) -> Option<&'a Vec<Token>> {
        Some(&self.iter.next()?.as_ref().unwrap().tokens)
    }
}

struct LineData {
    start_state: State,
    end_state: State,
    tokens: Vec<Token>,
}