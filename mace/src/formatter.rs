use {
    crate::{
        text::Text,
        token::{Delimiter, Punctuator, Token, TokenKind},
    },
    std::iter,
};

pub struct Formatter {
    lines: Vec<Vec<char>>,
    groups: Vec<Group>,
}

impl Formatter {
    pub fn new() -> Formatter {
        Formatter::default()
    }

    pub fn push_line(&mut self, chars: &[char], tokens: &[Token]) {
        let mut column = 0;
        for (index, token) in tokens.iter().enumerate() {
            match token.kind {
                TokenKind::Punctuator(Punctuator::Separator) => {
                    self.print_chars(&chars[column..][..token.len]);
                    let expand = self.groups.last().map_or(true, |group| group.expand);
                    if expand {
                        self.print_newline();
                    }
                }
                TokenKind::Punctuator(Punctuator::OpenDelimiter(delimiter)) => {
                    self.print_chars(&chars[column..][..token.len]);
                    let expand = tokens[index + 1..]
                        .iter()
                        .all(|token| token.kind == TokenKind::Whitespace);
                    self.groups.push(Group { expand, delimiter });
                    if expand {
                        self.print_newline();
                    }
                }
                TokenKind::Punctuator(Punctuator::CloseDelimiter(delimiter)) => {
                    let group = self.groups.pop().unwrap();
                    if group.delimiter != delimiter {
                        panic!();
                    }
                    if group.expand {
                        self.print_newline();
                    }
                    self.print_chars(&chars[column..][..token.len]);
                }
                TokenKind::Whitespace => {}
                _ => {
                    self.print_chars(&chars[column..][..token.len]);
                }
            }
            column += token.len;
        }
    }

    pub fn format(self) -> Text {
        Text::from_lines(self.lines)
    }

    fn print_chars(&mut self, chars: &[char]) {
        self.lines.last_mut().unwrap().extend(chars);
    }

    fn print_newline(&mut self) {
        self.lines.push(
            iter::repeat(' ')
                .take(self.groups.len() * 4)
                .collect::<Vec<_>>(),
        );
    }
}

impl Default for Formatter {
    fn default() -> Formatter {
        Formatter {
            lines: vec![vec![]],
            groups: Vec::default(),
        }
    }
}

struct Group {
    expand: bool,
    delimiter: Delimiter,
}
