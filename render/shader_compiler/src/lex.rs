use crate::ident::Ident;
use crate::lit::Lit;
use crate::token::Token;
use crate::ty_lit::TyLit;
use std::error;
use std::fmt;

#[derive(Clone, Debug)]
pub struct Lex<C> {
    chars: C,
    ch_0: char,
    ch_1: char,
    is_done: bool,
}

impl<C> Lex<C>
where
    C: Iterator<Item = char>,
{
    fn read_token(&mut self) -> Result<Token, Error> {
        loop {
            self.skip_chars_while(|ch| ch.is_ascii_whitespace());
            match (self.ch_0, self.ch_1) {
                ('/', '*') => {
                    self.skip_two_chars();
                    loop {
                        match (self.ch_0, self.ch_1) {
                            ('\0', _) => {
                                break Err(Error::UnterminatedBlockComment);
                            }
                            ('*', '/') => {
                                self.skip_two_chars();
                                break Ok(());
                            }
                            _ => {
                                self.skip_char();
                            }
                        }
                    }?
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
        match (self.ch_0, self.ch_1) {
            ('\0', _) => Ok(Token::Eof),
            ('!', '=') => {
                self.skip_two_chars();
                Ok(Token::NotEq)
            }
            ('!', _) => {
                self.skip_char();
                Ok(Token::Not)
            }
            ('&', '&') => {
                self.skip_two_chars();
                Ok(Token::AndAnd)
            }
            ('(', _) => {
                self.skip_char();
                Ok(Token::LeftParen)
            }
            (')', _) => {
                self.skip_char();
                Ok(Token::RightParen)
            }
            ('*', '=') => {
                self.skip_two_chars();
                Ok(Token::StarEq)
            }
            ('*', _) => {
                self.skip_char();
                Ok(Token::Star)
            }
            ('+', '=') => {
                self.skip_two_chars();
                Ok(Token::PlusEq)
            }
            ('+', _) => {
                self.skip_char();
                Ok(Token::Plus)
            }
            (',', _) => {
                self.skip_char();
                Ok(Token::Comma)
            }
            ('-', '=') => {
                self.skip_two_chars();
                Ok(Token::MinusEq)
            }
            ('-', '>') => {
                self.skip_two_chars();
                Ok(Token::Arrow)
            }
            ('-', _) => {
                self.skip_char();
                Ok(Token::Minus)
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
                        return Err(Error::MissingFloatExp);
                    }
                    true
                } else {
                    false
                };
                Ok(if has_frac_part || has_exp_part {
                    Token::Lit(Lit::Float(string.parse::<f32>().unwrap()))
                } else {
                    Token::Lit(Lit::Int(
                        string
                            .parse::<u32>()
                            .map_err(|_| Error::OverflowingIntLit)?,
                    ))
                })
            }
            ('.', _) => {
                self.skip_char();
                Ok(Token::Dot)
            }
            ('/', '=') => {
                self.skip_two_chars();
                Ok(Token::SlashEq)
            }
            ('/', _) => {
                self.skip_char();
                Ok(Token::Slash)
            }
            (':', _) => {
                self.skip_char();
                Ok(Token::Colon)
            }
            (';', _) => {
                self.skip_char();
                Ok(Token::Semi)
            }
            ('<', '=') => {
                self.skip_two_chars();
                Ok(Token::LtEq)
            }
            ('<', _) => {
                self.skip_char();
                Ok(Token::Lt)
            }
            ('=', '=') => {
                self.skip_two_chars();
                Ok(Token::EqEq)
            }
            ('=', _) => {
                self.skip_char();
                Ok(Token::Eq)
            }
            ('>', '=') => {
                self.skip_two_chars();
                Ok(Token::GtEq)
            }
            ('>', _) => {
                self.skip_char();
                Ok(Token::Gt)
            }
            ('?', _) => {
                self.skip_char();
                Ok(Token::Question)
            }
            (ch, _) if ch.is_ascii_alphabetic() || ch == '_' => {
                let mut string = String::new();
                string.push(self.read_char());
                self.read_chars_while(&mut string, |ch| ch.is_ascii_alphanumeric() || ch == '_');
                Ok(match string.as_str() {
                    "attribute" => Token::Attribute,
                    "bool" => Token::TyLit(TyLit::Bool),
                    "block" => Token::Block,
                    "break" => Token::Break,
                    "bvec2" => Token::TyLit(TyLit::Bvec2),
                    "bvec3" => Token::TyLit(TyLit::Bvec3),
                    "bvec4" => Token::TyLit(TyLit::Bvec4),
                    "continue" => Token::Continue,
                    "else" => Token::Else,
                    "false" => Token::Lit(Lit::Bool(false)),
                    "float" => Token::TyLit(TyLit::Float),
                    "fn" => Token::Fn,
                    "for" => Token::For,
                    "from" => Token::From,
                    "if" => Token::If,
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
                })
            }
            ('[', _) => {
                self.skip_char();
                Ok(Token::LeftBracket)
            }
            (']', _) => {
                self.skip_char();
                Ok(Token::RightBracket)
            }
            ('{', _) => {
                self.skip_char();
                Ok(Token::LeftBrace)
            }
            ('|', '|') => {
                self.skip_two_chars();
                Ok(Token::OrOr)
            }
            ('}', _) => {
                self.skip_char();
                Ok(Token::RightBrace)
            }
            _ => Err(Error::UnexpectedChar(self.ch_0)),
        }
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
    }

    fn skip_two_chars(&mut self) {
        self.ch_0 = self.chars.next().unwrap_or('\0');
        self.ch_1 = self.chars.next().unwrap_or('\0');
    }
}

impl<C> Iterator for Lex<C>
where
    C: Iterator<Item = char>,
{
    type Item = Result<Token, Error>;

    fn next(&mut self) -> Option<Result<Token, Error>> {
        if self.is_done {
            None
        } else {
            Some(self.read_token().map(|token| {
                if token == Token::Eof {
                    self.is_done = true
                }
                token
            }))
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Error {
    MissingFloatExp,
    OverflowingIntLit,
    UnexpectedChar(char),
    UnterminatedBlockComment,
}

impl error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::MissingFloatExp => write!(f, "missing float exponent"),
            Error::OverflowingIntLit => write!(f, "overflowing integer literal"),
            Error::UnexpectedChar(ch) => write!(f, "unexpected character {}", ch),
            Error::UnterminatedBlockComment => write!(f, "unterminated block comment"),
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
        is_done: false,
    }
}
