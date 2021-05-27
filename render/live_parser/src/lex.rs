use makepad_live_derive::*;
use crate::id::{IdMap, Id};
use crate::liveerror::LiveError;
use crate::liveerror::LiveErrorOrigin;
use crate::id::FileId;
use crate::span::Span;
use crate::token::{Token, TokenWithSpan};
use crate::colors::hex_bytes_to_u32;

#[derive(Clone, Debug)]
pub struct Lex<C> {
    chars: C,
    file_id: FileId,
    temp_string: String,
    temp_hex: Vec<u8>,
    strings: Vec<char>,
    group_stack: Vec<char>,
    ch_0: char,
    ch_1: char,
    index: usize,
    is_done: bool,
}

// put all the words here that the lexer might not see for collision check
pub fn fill_collisions() {
    IdMap::with(|idmap|{
        if idmap.contains("use"){
            return
        }
        let collision_seed = [
            "true",
            "false",
            "use",
            "!=",
            "!",
            "&&",
            "*=",
            "*",
            "+=",
            "+",
            ",",
            "-=",
            "->",
            "-",
            "..",
            ".",
            "/=",
            "/",
            "::",
            ":",
            ";",
            "<=",
            "<",
            "==",
            "=",
            ">=",
            ">",
            "?"
        ];
        for seed in &collision_seed{
            idmap.add(seed);
        }
    })
}

impl<C> Lex<C>
where
C: Iterator<Item = char>,
{
    
    fn read_token_with_span(&mut self) -> Result<TokenWithSpan, LiveError> {
        let span = self.begin_span();
        loop {
            self.skip_chars_while( | ch | ch.is_ascii_whitespace());
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
            ('"', _) => { // read a string
                self.skip_char();
                //let mut string = String::new();
                let start = self.strings.len();
                while let Some(ch) = self.read_char_if( | ch | ch != '"' && ch != '\0') {
                    self.strings.push(ch)
                }
                if self.ch_0 == '"' {
                    self.skip_char();
                }
                Token::String {
                    index: start as u32,
                    len: (self.strings.len() - start) as u32
                }
            }
            ('\0', _) => Token::Eof,
            ('!', '=') => {
                self.skip_two_chars();
                Token::Punct(id!( !=))
            }
            ('!', _) => {
                self.skip_char();
                Token::Punct(id!(!))
            }
            ('#', _) => {
                self.skip_char();
                self.temp_hex.truncate(0);
                while let Some(ch) = self.read_char_if( | ch | ch.is_ascii_hexdigit()) {
                    self.temp_hex.push(ch as u8)
                }
                
                if let Ok(color) = hex_bytes_to_u32(&self.temp_hex) {
                    Token::Color(color)
                }
                else {
                    return Err(span.error(self, "Cannot parse color".into()));
                }
            }
            ('&', '&') => {
                self.skip_two_chars();
                Token::Punct(id!( &&))
            }
            ('*', '=') => {
                self.skip_two_chars();
                Token::Punct(id!( *=))
            }
            ('*', _) => {
                self.skip_char();
                Token::Punct(id!(*))
            }
            ('+', '=') => {
                self.skip_two_chars();
                Token::Punct(id!( +=))
            }
            ('+', _) => {
                self.skip_char();
                Token::Punct(id!( +))
            }
            (',', _) => {
                self.skip_char();
                Token::Punct(id!(,))
            }
            ('-', '=') => {
                self.skip_two_chars();
                Token::Punct(id!( -=))
            }
            ('-', '>') => {
                self.skip_two_chars();
                Token::Punct(id!( ->))
            }
            ('-', '.') => {
                self.temp_string.truncate(0);
                self.skip_two_chars();
                self.temp_string.push('-');
                self.temp_string.push('0');
                self.temp_string.push('.');
                self.read_chars_while( | ch | ch.is_ascii_digit());
                Token::Float(self.temp_string.parse::<f64>().unwrap())
            }
            ('-', ch) | ('.', ch) | (ch, _) if ch.is_ascii_digit() => {
                self.temp_string.truncate(0);
                if self.ch_0 == '-' {
                    self.skip_char();
                    self.temp_string.push('-');
                }
                self.read_chars_while( | ch | ch.is_ascii_digit());
                let has_frac_part = if let Some(ch) = self.read_char_if( | ch | ch == '.') {
                    self.temp_string.push(ch);
                    self.read_chars_while( | ch | ch.is_ascii_digit());
                    true
                } else {
                    false
                };
                let has_exp_part = if let Some(ch) = self.read_char_if( | ch | ch == 'E' || ch == 'e')
                {
                    self.temp_string.push(ch);
                    if let Some(ch) = self.read_char_if( | ch | ch == '+' || ch == '-') {
                        self.temp_string.push(ch);
                    }
                    if let Some(ch) = self.read_char_if( | ch | ch.is_ascii_digit()) {
                        self.temp_string.push(ch);
                        self.read_chars_while( | ch | ch.is_ascii_digit());
                    } else {
                        return Err(span.error(self, "missing float exponent".into()));
                    }
                    true
                } else {
                    false
                };
                if has_frac_part || has_exp_part {
                    Token::Float(self.temp_string.parse::<f64>().unwrap())
                } else {
                    Token::Int(self.temp_string.parse::<i64>().map_err( | _ | {
                        span.error(self, "overflowing integer literal".into())
                    }) ?)
                }
            }
            ('-', _) => {
                self.skip_char();
                Token::Punct(id!(-))
            }
            ('.', '.') => {
                self.skip_two_chars();
                Token::Punct(id!(..))
            }
            ('.', _) => {
                self.skip_char();
                Token::Punct(id!(.))
            }
            ('/', '=') => {
                self.skip_two_chars();
                Token::Punct(id!( /=))
            }
            ('/', _) => {
                self.skip_char();
                Token::Punct(id!( /))
            }
            (':', ':') => {
                self.skip_two_chars();
                Token::Punct(id!(::))
            }
            (':', _) => {
                self.skip_char();
                Token::Punct(id!(:))
            }
            (';', _) => {
                self.skip_char();
                Token::Punct(id!(;))
            }
            ('<', '=') => {
                self.skip_two_chars();
                Token::Punct(id!( <=))
            }
            ('<', _) => {
                self.skip_char();
                Token::Punct(id!(<))
            }
            ('=', '=') => {
                self.skip_two_chars();
                Token::Punct(id!( ==))
            }
            ('=', _) => {
                self.skip_char();
                Token::Punct(id!( =))
            }
            ('>', '=') => {
                self.skip_two_chars();
                Token::Punct(id!( >=))
            }
            ('>', _) => {
                self.skip_char();
                Token::Punct(id!(>))
            }
            ('?', _) => {
                self.skip_char();
                Token::Punct(id!( ?))
            }
            (ch, _) if ch.is_ascii_alphabetic() || ch == '_' => {
                self.temp_string.truncate(0);
                let ch = self.read_char();
                self.temp_string.push(ch);
                self.read_chars_while( | ch | ch.is_ascii_alphanumeric() || ch == '_');
                match self.temp_string.as_str() {
                    "true" => Token::Bool(true),
                    "false" => Token::Bool(false),
                    _=>{
                        let id = Id::from_str(&self.temp_string);
                        if let Some(collide) = id.check_collision(&self.temp_string) {
                            return Err(span.error(self, format!("Id has collision {} with {}, please rename one of them", self.temp_string, collide).into()));
                        }
                        Token::Ident(id)
                    }
                }
            }
            ('(', _) => {
                self.skip_char();
                self.group_stack.push(')');
                Token::OpenParen
            }
            (')', _) => {
                if let Some(exp) = self.group_stack.pop() {
                    if exp != ')' {
                        return Err(span.error(self, format!("Expected {} but got )",exp).into()));
                    }
                }
                else {
                    return Err(span.error(self, "Got ) but no matching (".into()));
                }
                self.skip_char();
                Token::CloseParen
            }
            ('[', _) => {
                self.skip_char();
                self.group_stack.push(']');
                Token::OpenBracket
            }
            (']', _) => {
                if let Some(exp) = self.group_stack.pop() {
                    if exp != ']' {
                        return Err(span.error(self, format!("Expected {} but got ]",exp).into()));
                    }
                }
                else {
                    return Err(span.error(self, "Got ] but no matching [".into()));
                }
                self.skip_char();
                Token::CloseBracket
            }
            ('{', _) => {
                self.skip_char();
                self.group_stack.push('}');
                Token::OpenBrace
            }
            ('}', _) => {
               if let Some(exp) = self.group_stack.pop() {
                    if exp != '}' {
                        return Err(span.error(self, format!("Expected {} but got }}",exp).into()));
                    }
                }
                else {
                    return Err(span.error(self, "Got } but no matching {".into()));
                }
                self.skip_char();
                Token::CloseBrace
            }
            _ => {
                return Err(span.error(self, format!("unexpected character `{}`", self.ch_0).into()))
            }
        };
        Ok(span.token(self, token))
    }
    
    fn read_chars_while<P>(&mut self, mut pred: P)
    where
    P: FnMut(char) -> bool,
    {
        while let Some(ch) = self.read_char_if(&mut pred) {
            self.temp_string.push(ch);
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
            file_id: self.file_id,
            start: self.index,
        }
    }
}

impl<C> Iterator for Lex<C>
where
C: Iterator<Item = char>,
{
    type Item = Result<TokenWithSpan, LiveError>;
    
    fn next(&mut self) -> Option<Result<TokenWithSpan, LiveError >> {
        if self.is_done {
            None
        } else {
            Some(self.read_token_with_span().map( | token_with_span | {
                if token_with_span.token == Token::Eof {
                    self.is_done = true
                }
                token_with_span
            }))
        }
    }
}

pub struct LexResult{
    pub strings: Vec<char>,
    pub tokens: Vec<TokenWithSpan>
}

pub fn lex<C>(chars: C, file_id: FileId) -> Result<LexResult, LiveError>
where
C: IntoIterator<Item = char>,
{
    fill_collisions();
    let mut chars = chars.into_iter();
    let ch_0 = chars.next().unwrap_or('\0');
    let ch_1 = chars.next().unwrap_or('\0');
    let mut tokens = Vec::new();
    let mut lex = Lex {
        chars,
        ch_0,
        ch_1,
        file_id,
        index: 0,
        temp_hex: Vec::new(),
        temp_string: String::new(),
        group_stack: Vec::new(),
        strings: Vec::new(),
        is_done: false,
    };
    loop{
        match lex.read_token_with_span(){
            Err(err)=>{
                return Err(err)
            },
            Ok(tok)=>{
                tokens.push(tok);
                if tok.token == Token::Eof{
                    break
                }
            }
        }
    }
    return Ok(LexResult{
        strings: lex.strings,
        tokens
    });
}

struct SpanTracker {
    file_id: FileId,
    start: usize,
}

impl SpanTracker {
    fn token<C>(&self, lex: &Lex<C>, token: Token) -> TokenWithSpan {
        TokenWithSpan {
            span: Span::new(
                self.file_id,
                self.start,
                lex.index,
            ),
            token,
        }
    }
    
    fn error<C>(&self, lex: &Lex<C>, message: String) -> LiveError {
        LiveError {
            origin: live_error_origin!(),
            span: Span::new(
                self.file_id,
                self.start,
                 lex.index,
            ),
            message,
        }
    }
}
