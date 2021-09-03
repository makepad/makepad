use {
    crate::{
        delta::{Delta, OperationSpan},
        text::Text,
        token::{Token, TokenKind},
        tokenizer::{Cursor, State},
    },
    std::slice::Iter,
};

pub struct TokenCache {
    lines: Vec<Option<Line>>,
}

impl TokenCache {
    pub fn new(text: &Text) -> TokenCache {
        let mut tokenizer = TokenCache {
            lines: (0..text.as_lines().len()).map(|_| None).collect::<Vec<_>>(),
        };
        tokenizer.refresh(text);
        tokenizer
    }

    pub fn line_infos(&self) -> LineInfos<'_> {
        LineInfos {
            iter: self.lines.iter(),
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
                    self.lines[line] = None;
                    self.lines
                        .splice(line + 1..line + 1, (0..count.line).map(|_| None));
                    line += count.line;
                    if count.column > 0 {
                        self.lines[line] = None;
                    }
                }
                OperationSpan::Delete(count) => {
                    self.lines[line] = None;
                    self.lines.drain(line + 1..line + 1 + count.line);
                    if count.column > 0 {
                        self.lines[line] = None;
                    }
                }
            }
        }
    }

    pub fn refresh(&mut self, text: &Text) {
        let mut previous_line: Option<&Line> = None;
        let mut previous_line_did_change = false;
        for (index, line) in self.lines.iter_mut().enumerate() {
            if line.is_none() || previous_line_did_change {
                let mut state = previous_line
                    .map_or(State::default(), |previous_line| {
                        previous_line.end_state
                    });
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

                let indent_count = if tokens
                    .iter()
                    .any(|token| token.kind != TokenKind::Whitespace)
                {
                    let column = match tokens.first().unwrap() {
                        Token {
                            kind: TokenKind::Whitespace,
                            len,
                        } => *len,
                        _ => 0,
                    };
                    (column + 3) / 4
                } else {
                    previous_line.map_or(0, |previous_line| match previous_line.tokens.last() {
                        Some(Token {
                            kind: TokenKind::Punctuator(punctuator),
                            ..
                        }) if punctuator.is_left_delimiter() => previous_line.indent_count + 1,
                        _ => previous_line.indent_count,
                    })
                };

                let mut token_kinds_by_column = previous_line.map_or_else(
                    || Vec::new(),
                    |previous_line| previous_line.token_kinds_by_column.clone(),
                );
                if let Some((column, kind)) = tokens.iter().find_map({
                    let mut column = 0;
                    move |token| {
                        if token.kind != TokenKind::Whitespace {
                            return Some((column, token.kind));
                        }
                        column += token.len;
                        None
                    }
                }) {
                    while let Some((last_column, _)) = token_kinds_by_column.last() {
                        if *last_column < column {
                            break;
                        }
                        token_kinds_by_column.pop();
                    }
                    token_kinds_by_column.push((column, kind));
                }

                let new_line = Line {
                    tokens,
                    indent_count,
                    token_kinds_by_column,
                    end_state: state,
                };
                previous_line_did_change = line.as_ref() != Some(&new_line);
                *line = Some(new_line);
            }
            previous_line = Some(line.as_ref().unwrap());
        }
    }
}

pub struct LineInfos<'a> {
    iter: Iter<'a, Option<Line>>,
}

impl<'a> Iterator for LineInfos<'a> {
    type Item = LineInfo<'a>;

    fn next(&mut self) -> Option<LineInfo<'a>> {
        let line = self.iter.next()?.as_ref().unwrap();
        Some(LineInfo {
            tokens: &line.tokens,
            indent_count: line.indent_count,
            token_kinds_by_column: &line.token_kinds_by_column,
        })
    }
}

pub struct LineInfo<'a> {
    pub tokens: &'a [Token],
    pub indent_count: usize,
    pub token_kinds_by_column: &'a [(usize, TokenKind)],
}

#[derive(PartialEq)]
struct Line {
    tokens: Vec<Token>,
    indent_count: usize,
    token_kinds_by_column: Vec<(usize, TokenKind)>,
    end_state: State,
}