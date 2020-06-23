
use crate::error::Error;
use crate::ident::Ident;
use crate::lit::{Lit, TyLit};
use crate::span::Span;
use crate::token::{Token, TokenWithSpan};

#[derive(Clone, Debug)]
pub struct Lex<C> {
    chars: C,
    ch_0: char,
    ch_1: char,
    index: usize,
    is_done: bool,
}

impl<C> Lex<C>
where
    C: Iterator<Item = char>,
{
    fn read_token_with_span(&mut self) -> Result<TokenWithSpan, Error> {
        let span = self.begin_span();
        loop {
            self.skip_chars_while(|ch| ch.is_ascii_whitespace());
            match (self.ch_0, self.ch_1) {
                ('/', '*') => {
                    self.skip_two_chars();
                    loop {
                        match (self.ch_0, self.ch_1) {
                            ('\0', _) => {
                                return Err(span.error(self, "unterminated block comment".into()));
                            }
                            ('*', '/') => {
                                self.skip_two_chars();
                                break;
                            }
                            _ => {
                                self.skip_char();
                            }
                        }
                    }
                }
                ('/', '/') => {
                    self.skip_two_chars();
                    loop {
                        match (self.ch_0, self.ch_1) {
                            ('\n', _) => {
                                self.skip_char();
                                break;
                            }
                            ('\r', '\n') => {
                                self.skip_two_chars();
                                break;
                            }
                            _ => {
                                self.skip_char();
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        let span = self.begin_span();
        let token = match (self.ch_0, self.ch_1) {
            ('\0', _) => Token::Eof,
            ('!', '=') => {
                self.skip_two_chars();
                Token::NotEq
            }
            ('!', _) => {
                self.skip_char();
                Token::Not
            }
            ('&', '&') => {
                self.skip_two_chars();
                Token::AndAnd
            }
            ('(', _) => {
                self.skip_char();
                Token::LeftParen
            }
            (')', _) => {
                self.skip_char();
                Token::RightParen
            }
            ('*', '=') => {
                self.skip_two_chars();
                Token::StarEq
            }
            ('*', _) => {
                self.skip_char();
                Token::Star
            }
            ('+', '=') => {
                self.skip_two_chars();
                Token::PlusEq
            }
            ('+', _) => {
                self.skip_char();
                Token::Plus
            }
            (',', _) => {
                self.skip_char();
                Token::Comma
            }
            ('-', '=') => {
                self.skip_two_chars();
                Token::MinusEq
            }
            ('-', '>') => {
                self.skip_two_chars();
                Token::Arrow
            }
            ('-', _) => {
                self.skip_char();
                Token::Minus
            }
            ('.', ch) | (ch, _) if ch.is_ascii_digit() => {
                let mut string = String::new();
                self.read_chars_while(&mut string, |ch| ch.is_ascii_digit());
                let has_frac_part = if let Some(ch) = self.read_char_if(|ch| ch == '.') {
                    string.push(ch);
                    self.read_chars_while(&mut string, |ch| ch.is_ascii_digit());
                    true
                } else {
                    false
                };
                let has_exp_part = if let Some(ch) = self.read_char_if(|ch| ch == 'E' || ch == 'e')
                {
                    string.push(ch);
                    if let Some(ch) = self.read_char_if(|ch| ch == '+' || ch == '-') {
                        string.push(ch);
                    }
                    if let Some(ch) = self.read_char_if(|ch| ch.is_ascii_digit()) {
                        string.push(ch);
                        self.read_chars_while(&mut string, |ch| ch.is_ascii_digit());
                    } else {
                        return Err(span.error(self, "missing float exponent".into()));
                    }
                    true
                } else {
                    false
                };
                if has_frac_part || has_exp_part {
                    Token::Lit(Lit::Float(string.parse::<f32>().unwrap()))
                } else {
                    Token::Lit(Lit::Int(
                        string
                            .parse::<u32>()
                            .map_err(|_| span.error(self, "overflowing integer literal".into()))?,
                    ))
                }
            }
            ('.', _) => {
                self.skip_char();
                Token::Dot
            }
            ('/', '=') => {
                self.skip_two_chars();
                Token::SlashEq
            }
            ('/', _) => {
                self.skip_char();
                Token::Slash
            }
            (':', ':') => {
                self.skip_two_chars();
                Token::PathSep
            }
            (':', _) => {
                self.skip_char();
                Token::Colon
            }
            (';', _) => {
                self.skip_char();
                Token::Semi
            }
            ('<', '=') => {
                self.skip_two_chars();
                Token::LtEq
            }
            ('<', _) => {
                self.skip_char();
                Token::Lt
            }
            ('=', '=') => {
                self.skip_two_chars();
                Token::EqEq
            }
            ('=', _) => {
                self.skip_char();
                Token::Eq
            }
            ('>', '=') => {
                self.skip_two_chars();
                Token::GtEq
            }
            ('>', _) => {
                self.skip_char();
                Token::Gt
            }
            ('?', _) => {
                self.skip_char();
                Token::Question
            }
            (ch, _) if ch.is_ascii_alphabetic() || ch == '_' => {
                let mut string = String::new();
                string.push(self.read_char());
                self.read_chars_while(&mut string, |ch| ch.is_ascii_alphanumeric() || ch == '_');
                match string.as_str() {
                    "attribute" => Token::Attribute,
                    "bool" => Token::TyLit(TyLit::Bool),
                    "break" => Token::Break,
                    "bvec2" => Token::TyLit(TyLit::Bvec2),
                    "bvec3" => Token::TyLit(TyLit::Bvec3),
                    "bvec4" => Token::TyLit(TyLit::Bvec4),
                    "const" => Token::Const,
                    "continue" => Token::Continue,
                    "else" => Token::Else,
                    "false" => Token::Lit(Lit::Bool(false)),
                    "float" => Token::TyLit(TyLit::Float),
                    "fn" => Token::Fn,
                    "for" => Token::For,
                    "from" => Token::From,
                    "if" => Token::If,
                    "in" => Token::In,
                    "instance" => Token::Instance,
                    "int" => Token::TyLit(TyLit::Int),
                    "ivec2" => Token::TyLit(TyLit::Ivec2),
                    "ivec3" => Token::TyLit(TyLit::Ivec3),
                    "ivec4" => Token::TyLit(TyLit::Ivec4),
                    "let" => Token::Let,
                    "mat2" => Token::TyLit(TyLit::Mat2),
                    "mat3" => Token::TyLit(TyLit::Mat3),
                    "mat4" => Token::TyLit(TyLit::Mat4),
                    "return" => Token::Return,
                    "step" => Token::Step,
                    "struct" => Token::Struct,
                    "to" => Token::To,
                    "uniform" => Token::Uniform,
                    "varying" => Token::Varying,
                    "vec2" => Token::TyLit(TyLit::Vec2),
                    "vec3" => Token::TyLit(TyLit::Vec3),
                    "vec4" => Token::TyLit(TyLit::Vec4),
                    "true" => Token::Lit(Lit::Bool(true)),
                    _ => Token::Ident(Ident::new(string)),
                }
            }
            ('[', _) => {
                self.skip_char();
                Token::LeftBracket
            }
            (']', _) => {
                self.skip_char();
                Token::RightBracket
            }
            ('{', _) => {
                self.skip_char();
                Token::LeftBrace
            }
            ('|', '|') => {
                self.skip_two_chars();
                Token::OrOr
            }
            ('}', _) => {
                self.skip_char();
                Token::RightBrace
            }
            _ => return Err(span.error(self, format!("unexpected character `{}`", self.ch_0).into())),
        };
        Ok(span.token(self, token))
    }

    fn read_chars_while<P>(&mut self, string: &mut String, mut pred: P)
    where
        P: FnMut(char) -> bool,
    {
        while let Some(ch) = self.read_char_if(&mut pred) {
            string.push(ch);
        }
    }

    fn read_char_if<P>(&mut self, pred: P) -> Option<char>
    where
        P: FnOnce(char) -> bool,
    {
        if pred(self.ch_0) {
            Some(self.read_char())
        } else {
            None
        }
    }

    fn read_char(&mut self) -> char {
        let ch = self.ch_0;
        self.skip_char();
        ch
    }

    fn skip_chars_while<P>(&mut self, mut pred: P)
    where
        P: FnMut(char) -> bool,
    {
        while self.skip_char_if(&mut pred) {}
    }

    fn skip_char_if<P>(&mut self, pred: P) -> bool
    where
        P: FnOnce(char) -> bool,
    {
        if pred(self.ch_0) {
            self.skip_char();
            true
        } else {
            false
        }
    }

    fn skip_char(&mut self) {
        self.ch_0 = self.ch_1;
        self.ch_1 = self.chars.next().unwrap_or('\0');
        self.index += 1;
    }

    fn skip_two_chars(&mut self) {
        self.ch_0 = self.chars.next().unwrap_or('\0');
        self.ch_1 = self.chars.next().unwrap_or('\0');
        self.index += 2;
    }

    fn begin_span(&mut self) -> SpanTracker {
        SpanTracker {
            start: self.index
        }
    }
}

impl<C> Iterator for Lex<C>
where
    C: Iterator<Item = char>,
{
    type Item = Result<TokenWithSpan, Error>;

    fn next(&mut self) -> Option<Result<TokenWithSpan, Error>> {
        if self.is_done {
            None
        } else {
            Some(self.read_token_with_span().map(|token_with_span| {
                if token_with_span.token == Token::Eof {
                    self.is_done = true
                }
                token_with_span
            }))
        }
    }
}

pub fn lex<C>(chars: C) -> Lex<C::IntoIter>
where
    C: IntoIterator<Item = char>,
{
    let mut chars = chars.into_iter();
    let ch_0 = chars.next().unwrap_or('\0');
    let ch_1 = chars.next().unwrap_or('\0');
    Lex {
        chars,
        ch_0,
        ch_1,
        index: 0,
        is_done: false,
    }
}

struct SpanTracker {
    start: usize
}

impl SpanTracker {
    fn token<C>(&self, lex: &Lex<C>, token: Token) -> TokenWithSpan {
        TokenWithSpan {
            span: Span {
                start: self.start,
                end: lex.index,
            },
            token,
        }
    }

    fn error<C>(&self, lex: &Lex<C>, message: String) -> Error {
        Error {
            span: Span {
                start: self.start,
                end: lex.index,
            },
            message,
        }
    }
}