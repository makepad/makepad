use crate::{
    delta::{Delta, OperationRange},
    text::Text,
    token::{Token, TokenKind},
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
        self.refresh_token_info(text);
        self.refresh_leading_whitespace_above();
        self.refresh_leading_whitespace_below();
    }

    fn refresh_token_info(&mut self, text: &Text) {
        let mut previous_line_info: Option<&LineInfo> = None;
        let mut previous_did_change = false;
        for (index, line_info) in self.line_infos.iter_mut().enumerate() {
            if previous_did_change || line_info.token_info.is_none() {
                let mut state = previous_line_info
                    .map(|previous_line_info| previous_line_info.end_state())
                    .unwrap_or_default();
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
                let new_token_info = TokenInfo {
                    tokens,
                    end_state: state,
                };
                let did_change = line_info.token_info.as_ref() != Some(&new_token_info);
                if did_change {
                    line_info.token_info = Some(new_token_info);
                }
                previous_did_change = did_change;
            }
            previous_line_info = Some(line_info);
        }
    }

    fn refresh_leading_whitespace_above(&mut self) {
        let mut previous_line_info: Option<&LineInfo> = None;
        let mut previous_did_change = false;
        for line_info in &mut self.line_infos {
            if previous_did_change || line_info.leading_whitespace_above.is_none() {
                let new_leading_whitespace_above = line_info
                    .tokens()
                    .iter()
                    .find_map({
                        let mut new_leading_whitespace_above = 0;
                        move |token| {
                            if token.kind != TokenKind::Whitespace {
                                return Some(new_leading_whitespace_above);
                            }
                            new_leading_whitespace_above += token.len;
                            None
                        }
                    })
                    .unwrap_or_else(|| {
                        previous_line_info
                            .map(|previous_line_info| previous_line_info.leading_whitespace_above())
                            .unwrap_or_default()
                    });
                let did_change =
                    line_info.leading_whitespace_above != Some(new_leading_whitespace_above);
                if did_change {
                    line_info.leading_whitespace_above = Some(new_leading_whitespace_above);
                }
                previous_did_change = did_change;
            }
            previous_line_info = Some(line_info);
        }
    }

    fn refresh_leading_whitespace_below(&mut self) {
        let mut previous_line_info: Option<&LineInfo> = None;
        let mut previous_did_change = false;
        for line_info in self.line_infos.iter_mut().rev() {
            if previous_did_change || line_info.leading_whitespace_below.is_none() {
                let new_leading_whitespace_below = line_info
                    .tokens()
                    .iter()
                    .find_map({
                        let mut new_leading_whitespace_below = 0;
                        move |token| {
                            if token.kind != TokenKind::Whitespace {
                                return Some(new_leading_whitespace_below);
                            }
                            new_leading_whitespace_below += token.len;
                            None
                        }
                    })
                    .unwrap_or_else(|| {
                        previous_line_info
                            .map(|previous_line_info| previous_line_info.leading_whitespace_below())
                            .unwrap_or_default()
                    });
                let did_change =
                    line_info.leading_whitespace_below != Some(new_leading_whitespace_below);
                if did_change {
                    line_info.leading_whitespace_below = Some(new_leading_whitespace_below);
                }
                previous_did_change = did_change;
            }
            previous_line_info = Some(line_info);
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct LineInfo {
    token_info: Option<TokenInfo>,
    leading_whitespace_above: Option<usize>,
    leading_whitespace_below: Option<usize>,
}

impl LineInfo {
    pub fn tokens(&self) -> &[Token] {
        &self.token_info.as_ref().unwrap().tokens
    }

    fn end_state(&self) -> State {
        self.token_info.as_ref().unwrap().end_state
    }

    pub fn leading_whitespace(&self) -> usize {
        self.leading_whitespace_above()
            .min(self.leading_whitespace_below())
    }

    fn leading_whitespace_above(&self) -> usize {
        self.leading_whitespace_above.unwrap()
    }

    fn leading_whitespace_below(&self) -> usize {
        self.leading_whitespace_below.unwrap()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TokenInfo {
    tokens: Vec<Token>,
    end_state: State,
}
