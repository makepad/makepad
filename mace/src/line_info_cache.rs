use crate::{
    delta::{Delta, OperationRange},
    text::Text,
    token::Token,
    tokenizer::{Cursor, State},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LineInfoCache {
    line_infos: Vec<LineInfo>,
}

impl LineInfoCache {
    pub fn new(text: &Text) -> LineInfoCache {
        let mut cache = LineInfoCache {
            line_infos: (0..text.as_lines().len())
                .map(|_| LineInfo::default())
                .collect::<Vec<_>>(),
        };
        cache.refresh(text);
        cache
    }

    pub fn line_infos(&self) -> &[LineInfo] {
        &self.line_infos
    }

    pub fn invalidate(&mut self, delta: &Delta) {
        for operation_range in delta.operation_ranges() {
            match operation_range {
                OperationRange::Insert(range) => {
                    self.line_infos[range.start.line] = LineInfo::default();
                    self.line_infos.splice(
                        range.start.line + 1..range.start.line + 1,
                        (0..range.end.line - range.start.line).map(|_| LineInfo::default()),
                    );
                }
                OperationRange::Delete(range) => {
                    self.line_infos.drain(range.start.line..range.end.line);
                    self.line_infos[range.start.line] = LineInfo::default();
                }
            }
        }
    }

    pub fn refresh(&mut self, text: &Text) {
        let mut state = State::default();
        for (index, line_info) in self.line_infos.iter_mut().enumerate() {
            match line_info.token_info {
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
                    line_info.token_info = Some(TokenInfo {
                        start_state,
                        tokens,
                        end_state: state,
                    });
                }
            }
        }

        for (index, line_info) in self.line_infos.iter_mut().enumerate() {
            if line_info.non_whitespace_start.is_some() {
                continue;
            }
            line_info.non_whitespace_start = Some(
                text.as_lines()[index]
                    .iter()
                    .position(|ch| !ch.is_whitespace()),
            );
        }

        let mut leading_whitespace_above = 0;
        for line_info in self.line_infos.iter_mut() {
            if line_info.contains_non_whitespace() {
                leading_whitespace_above = line_info.leading_whitespace();
            }
            line_info.leading_whitespace_above = Some(leading_whitespace_above);
        }

        let mut leading_whitespace_below = 0;
        for line_info in self.line_infos.iter_mut().rev() {
            if line_info.contains_non_whitespace() {
                leading_whitespace_below = line_info.leading_whitespace();
            }
            line_info.leading_whitespace_below = Some(leading_whitespace_below);
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct LineInfo {
    token_info: Option<TokenInfo>,
    non_whitespace_start: Option<Option<usize>>,
    leading_whitespace_above: Option<usize>,
    leading_whitespace_below: Option<usize>,
}

impl LineInfo {
    pub fn tokens(&self) -> &[Token] {
        &self.token_info.as_ref().unwrap().tokens
    }

    fn non_whitespace_start(&self) -> Option<usize> {
        self.non_whitespace_start.unwrap()
    }

    fn contains_non_whitespace(&self) -> bool {
        self.non_whitespace_start().is_some()
    }

    fn leading_whitespace(&self) -> usize {
        self.non_whitespace_start().unwrap_or(0)
    }

    fn leading_whitespace_above(&self) -> usize {
        self.leading_whitespace_above.unwrap()
    }

    fn leading_whitespace_below(&self) -> usize {
        self.leading_whitespace_below.unwrap()
    }

    pub fn virtual_leading_whitespace(&self) -> usize {
        self.leading_whitespace_above()
            .min(self.leading_whitespace_below())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TokenInfo {
    start_state: State,
    tokens: Vec<Token>,
    end_state: State,
}
