use std::collections::{HashMap};
use std::hash::Hash;
use std::str::Chars;

pub struct SerRonState {
    pub out: String
}

impl SerRonState {
    pub fn indent(&mut self, d: usize) {
        for _ in 0..d {
            self.out.push_str("    ");
        }
    }
    
    pub fn field(&mut self, d: usize, field: &str) {
        self.indent(d);
        self.out.push_str(field);
        self.out.push(':');
    }
    
    pub fn conl(&mut self) {
        self.out.push_str(",\n")
    }
    
    pub fn st_pre(&mut self) {
        self.out.push_str("(\n");
    }
    
    pub fn st_post(&mut self, d: usize) {
        self.indent(d);
        self.out.push(')');
    }
    
}

pub trait SerRon {
    
    fn serialize_ron(&self) -> String {
        let mut s = SerRonState {
            out: String::new()
        };
        self.ser_ron(0, &mut s);
        s.out
    }
    
    fn ser_ron(&self, d: usize, s: &mut SerRonState);
}

pub trait DeRon: Sized {
    
    fn deserialize_ron(input: &str) -> Result<Self,
    DeRonErr> {
        let mut state = DeRonState::default();
        let mut chars = input.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars) ?;
        DeRon::de_ron(&mut state, &mut chars)
    }
    
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
    DeRonErr>;
}

#[derive(PartialEq, Debug)]
pub enum DeRonTok {
    Ident,
    Str,
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    Char(char),
    Colon,
    CurlyOpen,
    CurlyClose,
    ParenOpen,
    ParenClose,
    BlockOpen,
    BlockClose,
    Comma,
    Bof,
    Eof
}

impl Default for DeRonTok {
    fn default() -> Self {DeRonTok::Bof}
}

#[derive(Default)]
pub struct DeRonState {
    pub cur: char,
    pub tok: DeRonTok,
    pub strbuf: String,
    pub numbuf: String,
    pub identbuf: String,
    pub line: usize,
    pub col: usize
}

pub struct DeRonErr {
    pub msg: String,
    pub line: usize,
    pub col: usize
}

impl std::fmt::Debug for DeRonErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ron Deserialize error: {}, line:{} col:{}", self.msg, self.line + 1, self.col + 1)
    }
}

impl DeRonState {
    pub fn next(&mut self, i: &mut Chars) {
        if let Some(c) = i.next() {
            self.cur = c;
            if self.cur == '\n' {
                self.line += 1;
                self.col = 0;
            }
            else {
                self.col = 0;
            }
        }
        else {
            self.cur = '\0';
        }
    }
    
    pub fn err_exp(&self, name: &str) -> DeRonErr {
        DeRonErr {msg: format!("Unexpected key {}", name), line: self.line, col: self.col}
    }
    
    pub fn err_nf(&self, name: &str) -> DeRonErr {
        DeRonErr {msg: format!("Key not found {}", name), line: self.line, col: self.col}
    }
    
    pub fn err_enum(&self, name: &str) -> DeRonErr {
        DeRonErr {msg: format!("Enum not defined {}", name), line: self.line, col: self.col}
    }
    
    pub fn err_token(&self, what: &str) -> DeRonErr {
        DeRonErr {msg: format!("Unexpected token {:?} expected {} ", self.tok, what), line: self.line, col: self.col}
    }
    
    pub fn err_range(&self, what: &str) -> DeRonErr {
        DeRonErr {msg: format!("Value out of range {} ", what), line: self.line, col: self.col}
    }
    
    pub fn err_type(&self, what: &str) -> DeRonErr {
        DeRonErr {msg: format!("Token wrong type {} ", what), line: self.line, col: self.col}
    }
    
    pub fn err_parse(&self, what: &str) -> DeRonErr {
        DeRonErr {msg: format!("Cannot parse {} ", what), line: self.line, col: self.col}
    }
    
    pub fn eat_comma_paren(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        match self.tok {
            DeRonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeRonTok::ParenClose => {
                Ok(())
            }
            _ => {
                Err(self.err_token(", or )"))
            }
        }
    }
    
    pub fn eat_comma_block(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        match self.tok {
            DeRonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeRonTok::BlockClose => {
                Ok(())
            }
            _ => {
                Err(self.err_token(", or ]"))
            }
        }
    }
    
    pub fn eat_comma_curly(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        match self.tok {
            DeRonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeRonTok::CurlyClose => {
                Ok(())
            }
            _ => {
                Err(self.err_token(", or }"))
            }
        }
    }
    
    pub fn colon(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        match self.tok {
            DeRonTok::Colon => {
                self.next_tok(i) ?;
                Ok(())
            },
            _ => {
                Err(self.err_token(":"))
            }
        }
    }
    
    pub fn ident(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        match &mut self.tok {
            DeRonTok::Ident => {
                self.next_tok(i) ?;
                Ok(())
            },
            _ => {
                Err(self.err_token("Identifier"))
            }
        }
    }
    
    pub fn next_colon(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        self.next_tok(i) ?;
        self.colon(i) ?;
        Ok(())
    }
    
    pub fn next_ident(&mut self) -> Option<()> {
        if let DeRonTok::Ident = &mut self.tok {
            Some(())
        }
        else {
            None
        }
    }
    
    pub fn paren_open(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        if self.tok == DeRonTok::ParenOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("("))
    }
    
    
    pub fn paren_close(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        if self.tok == DeRonTok::ParenClose {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token(")"))
    }
    
    pub fn block_open(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        if self.tok == DeRonTok::BlockOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("["))
    }
    
    
    pub fn block_close(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        if self.tok == DeRonTok::BlockClose {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("]"))
    }
    
    pub fn curly_open(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        if self.tok == DeRonTok::CurlyOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("{"))
    }
    
    
    pub fn curly_close(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        if self.tok == DeRonTok::CurlyClose {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("}"))
    }
    
    
    pub fn u64_range(&mut self, max: u64) -> Result<u64, DeRonErr> {
        if let DeRonTok::U64(value) = self.tok {
            if value > max {
                return Err(self.err_range(&format!("{}>{}", value, max)))
            }
            return Ok(value)
        }
        Err(self.err_token("unsigned integer"))
    }
    
    pub fn i64_range(&mut self, min: i64, max: i64) -> Result<i64, DeRonErr> {
        if let DeRonTok::I64(value) = self.tok {
            if value< min {
                return Err(self.err_range(&format!("{}<{}", value, min)))
            }
            return Ok(value)
        }
        if let DeRonTok::U64(value) = self.tok {
            if value as i64 > max {
                return Err(self.err_range(&format!("{}>{}", value, max)))
            }
            return Ok(value as i64)
        }
        Err(self.err_token("signed integer"))
    }
    
    pub fn as_f64(&mut self) -> Result<f64, DeRonErr> {
        if let DeRonTok::I64(value) = self.tok {
            return Ok(value as f64)
        }
        if let DeRonTok::U64(value) = self.tok {
            return Ok(value as f64)
        }
        if let DeRonTok::F64(value) = self.tok {
            return Ok(value)
        }
        Err(self.err_token("floating point"))
    }
    
    pub fn as_bool(&mut self) -> Result<bool, DeRonErr> {
        if let DeRonTok::Bool(value) = self.tok {
            return Ok(value)
        }
        if let DeRonTok::U64(value) = self.tok {
            return Ok(value != 0)
        }
        Err(self.err_token("boolean"))
    }
    
    pub fn as_string(&mut self) -> Result<String, DeRonErr> {
        if let DeRonTok::Str = &mut self.tok {
            let mut val = String::new();
            std::mem::swap(&mut val, &mut self.strbuf);
            return Ok(val)
        }
        Err(self.err_token("string"))
    }
    
    pub fn next_tok(&mut self, i: &mut Chars) -> Result<(), DeRonErr> {
        loop {
            while self.cur == '\n' || self.cur == '\r' || self.cur == '\t' || self.cur == ' ' {
                self.next(i);
            }
            match self.cur {
                '\0' => {
                    self.tok = DeRonTok::Eof;
                    return Ok(())
                },
                ':' => {
                    self.next(i);
                    self.tok = DeRonTok::Colon;
                    return Ok(())
                }
                ',' => {
                    self.next(i);
                    self.tok = DeRonTok::Comma;
                    return Ok(())
                }
                '[' => {
                    self.next(i);
                    self.tok = DeRonTok::BlockOpen;
                    return Ok(())
                }
                ']' => {
                    self.next(i);
                    self.tok = DeRonTok::BlockClose;
                    return Ok(())
                }
                '(' => {
                    self.next(i);
                    self.tok = DeRonTok::ParenOpen;
                    return Ok(())
                }
                ')' => {
                    self.next(i);
                    self.tok = DeRonTok::ParenClose;
                    return Ok(())
                }
                '{' => {
                    self.next(i);
                    self.tok = DeRonTok::CurlyOpen;
                    return Ok(())
                }
                '}' => {
                    self.next(i);
                    self.tok = DeRonTok::CurlyClose;
                    return Ok(())
                }
                '/' => {
                    self.next(i);
                    if self.cur == '/' { // single line comment
                        while self.cur != '\0' {
                            if self.cur == '\n' {
                                self.next(i);
                                break;
                            }
                            self.next(i);
                        }
                    }
                    else if self.cur == '*' { // multline comment
                        let mut last_star = false;
                        while self.cur != '\0' {
                            if self.cur == '/' && last_star {
                                self.next(i);
                                break;
                            }
                            if self.cur == '*' {last_star = true}else {last_star = false}
                            self.next(i);
                        }
                    }
                    else {
                        return Err(self.err_parse("comment"));
                    }
                },
                '-' | '0'..='9' => {
                    self.numbuf.clear();
                    let is_neg = if self.cur == '-' {
                        self.numbuf.push(self.cur);
                        self.next(i);
                        true
                    }
                    else {
                        false
                    };
                    while self.cur >= '0' && self.cur <= '9' {
                        self.numbuf.push(self.cur);
                        self.next(i);
                    }
                    if self.cur == '.' {
                        self.numbuf.push(self.cur);
                        self.next(i);
                        while self.cur >= '0' && self.cur <= '9' {
                            self.numbuf.push(self.cur);
                            self.next(i);
                        }
                        if let Ok(num) = self.numbuf.parse() {
                            self.tok = DeRonTok::F64(num);
                            return Ok(())
                        }
                        else {
                            return Err(self.err_parse("number"));
                        }
                    }
                    else {
                        if is_neg {
                            if let Ok(num) = self.numbuf.parse() {
                                self.tok = DeRonTok::I64(num);
                                return Ok(())
                            }
                            else {
                                return Err(self.err_parse("number"));
                            }
                        }
                        if let Ok(num) = self.numbuf.parse() {
                            self.tok = DeRonTok::U64(num);
                            return Ok(())
                        }
                        else {
                            return Err(self.err_parse("number"));
                        }
                    }
                },
                'a'..='z' | 'A'..='Z' | '_' => {
                    self.identbuf.clear();
                    while self.cur >= 'a' && self.cur <= 'z'
                        || self.cur >= 'A' && self.cur <= 'Z'
                        || self.cur == '_' {
                        self.identbuf.push(self.cur);
                        self.next(i);
                    }
                    if self.identbuf == "true" {
                        self.tok = DeRonTok::Bool(true);
                        return Ok(())
                    }
                    if self.identbuf == "false" {
                        self.tok = DeRonTok::Bool(false);
                        return Ok(())
                    }
                    self.tok = DeRonTok::Ident;
                    return Ok(())
                },
                '\'' => {
                    self.next(i);
                    if self.cur == '\\' {
                        self.next(i);
                    }
                    let chr = self.cur;
                    self.next(i);
                    if self.cur != '\'' {
                        return Err(self.err_token("char"));
                    }
                    self.next(i);
                    self.tok = DeRonTok::Char(chr);
                },
                '"' => {
                    self.strbuf.clear();
                    self.next(i);
                    while self.cur != '"' {
                        if self.cur == '\\' {
                            self.next(i);
                            match self.cur {
                                'n' => self.strbuf.push('\n'),
                                'r' => self.strbuf.push('\r'),
                                't' => self.strbuf.push('\t'),
                                '0' => self.strbuf.push('\0'),
                                '\0' => {
                                    return Err(self.err_parse("string"));
                                },
                                _ => self.strbuf.push(self.cur)
                            }
                            self.next(i);
                        }
                        else{
                            if self.cur == '\0' {
                                return Err(self.err_parse("string"));
                            }
                            self.strbuf.push(self.cur);
                            self.next(i);
                        }
                    }
                    self.next(i);
                    self.tok = DeRonTok::Str;
                    return Ok(())
                },
                _ => {
                    return Err(self.err_token("tokenizer"));
                }
            }
        }
    }
}

macro_rules!impl_ser_de_ron_unsigned {
    ( $ ty: ident, $ max: expr) => {
        impl SerRon for $ ty {
            fn ser_ron(&self, _d: usize, s: &mut SerRonState) {
                s.out.push_str(&self.to_string());
            }
        }
        
        impl DeRon for $ ty {
            fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result< $ ty,
            DeRonErr> {
                //s.is_prefix(p, i) ?;
                let val = s.u64_range( $ max as u64) ?;
                s.next_tok(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

macro_rules!impl_ser_de_ron_signed {
    ( $ ty: ident, $ min: expr, $ max: expr) => {
        impl SerRon for $ ty {
            fn ser_ron(&self, _d: usize, s: &mut SerRonState) {
                s.out.push_str(&self.to_string());
            }
        }
        
        impl DeRon for $ ty {
            fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result< $ ty,
            DeRonErr> {
                //s.is_prefix(p, i) ?;
                let val = s.i64_range( $ min as i64, $ max as i64) ?;
                s.next_tok(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

macro_rules!impl_ser_de_ron_float {
    ( $ ty: ident) => {
        impl SerRon for $ ty {
            fn ser_ron(&self, _d: usize, s: &mut SerRonState) {
                s.out.push_str(&self.to_string());
            }
        }
        
        impl DeRon for $ ty {
            fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result< $ ty,
            DeRonErr> {
                //s.is_prefix(p, i) ?;
                let val = s.as_f64() ?;
                s.next_tok(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

impl_ser_de_ron_unsigned!(usize, std::u64::MAX);
impl_ser_de_ron_unsigned!(u64, std::u64::MAX);
impl_ser_de_ron_unsigned!(u32, std::u32::MAX);
impl_ser_de_ron_unsigned!(u16, std::u16::MAX);
impl_ser_de_ron_unsigned!(u8, std::u8::MAX);
impl_ser_de_ron_signed!(i64, std::i64::MIN, std::i64::MAX);
impl_ser_de_ron_signed!(i32, std::i64::MIN, std::i64::MAX);
impl_ser_de_ron_signed!(i16, std::i64::MIN, std::i64::MAX);
impl_ser_de_ron_signed!(i8, std::i64::MIN, std::i8::MAX);
impl_ser_de_ron_float!(f64);
impl_ser_de_ron_float!(f32);

impl<T> SerRon for Option<T> where T: SerRon {
    fn ser_ron(&self, d: usize, s: &mut SerRonState) {
        if let Some(v) = self {
            v.ser_ron(d, s);
        }
        else {
            s.out.push_str("None");
        }
    }
}

impl<T> DeRon for Option<T> where T: DeRon {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
    DeRonErr> {
        if let DeRonTok::Ident = &s.tok {
            if s.identbuf == "None" {
                s.next_tok(i) ?;
                return Ok(None)
            }
        }
        Ok(Some(DeRon::de_ron(s, i) ?))
    }
}

impl SerRon for bool {
    fn ser_ron(&self, _d: usize, s: &mut SerRonState) {
        if *self {
            s.out.push_str("true")
        }
        else {
            s.out.push_str("false")
        }
    }
}

impl DeRon for bool {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<bool, DeRonErr> {
        let val = s.as_bool() ?;
        s.next_tok(i) ?;
        return Ok(val);
    }
}

impl SerRon for String {
    fn ser_ron(&self, _d: usize, s: &mut SerRonState) {
        s.out.push('"');
        for c in self.chars() {
            match c {
                '\n' => {s.out.push('\\'); s.out.push('n');},
                '\r' => {s.out.push('\\'); s.out.push('r');},
                '\t' => {s.out.push('\\'); s.out.push('t');},
                '\0' => {s.out.push('\\'); s.out.push('0');},
                '\\' => {s.out.push('\\'); s.out.push('\\');},
                '"' => {s.out.push('\\'); s.out.push('"');},
                _ => s.out.push(c)
            }
        }
        s.out.push('"');
    }
}

impl DeRon for String {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<String, DeRonErr> {
        let val = s.as_string() ?;
        s.next_tok(i) ?;
        return Ok(val);
    }
}

impl<T> SerRon for Vec<T> where T: SerRon {
    fn ser_ron(&self, d: usize, s: &mut SerRonState) {
        s.out.push_str("[\n");
        for item in self {
            s.indent(d + 1);
            item.ser_ron(d + 1, s);
            s.conl();
        }
        s.indent(d);
        s.out.push(']');
    }
}

impl<T> DeRon for Vec<T> where T: DeRon {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Vec<T>, DeRonErr> {
        let mut out = Vec::new();
        s.block_open(i) ?;
        
        while s.tok != DeRonTok::BlockClose {
            out.push(DeRon::de_ron(s, i) ?);
            s.eat_comma_block(i) ?;
        }
        s.block_close(i) ?;
        Ok(out)
    }
}

impl<T> SerRon for [T] where T: SerRon {
    fn ser_ron(&self, d: usize, s: &mut SerRonState) {
        s.out.push('(');
        let last = self.len() - 1;
        for (index, item) in self.iter().enumerate() {
            item.ser_ron(d + 1, s);
            if index != last {
                s.out.push_str(", ");
            }
        }
        s.out.push(')');
    }
}

unsafe fn de_ron_array_impl_inner<T>(top: *mut T, count: usize, s: &mut DeRonState, i: &mut Chars) -> Result<(), DeRonErr> where T: DeRon {
    s.paren_open(i) ?;
    for c in 0..count {
        top.add(c).write(DeRon::de_ron(s, i) ?);
        s.eat_comma_paren(i) ?;
    }
    s.paren_close(i) ?;
    Ok(())
}

macro_rules!de_ron_array_impl {
    ( $ ( $ count: expr), *) => {
        $ (
            impl<T> DeRon for [T; $ count] where T: DeRon {
                fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
                DeRonErr> {
                    unsafe {
                        let mut to = std::mem::MaybeUninit::<[T; $ count]>::uninit();
                        let top: *mut T = std::mem::transmute(&mut to);
                        de_ron_array_impl_inner(top, $ count, s, i) ?;
                        Ok(to.assume_init())
                    }
                }
            }
        ) *
    }
}

de_ron_array_impl!(2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32);

fn de_ron_comma_paren<T>(s: &mut DeRonState, i: &mut Chars) -> Result<T, DeRonErr> where T: DeRon {
    let t = DeRon::de_ron(s, i);
    s.eat_comma_paren(i) ?;
    t
}

impl<A, B> SerRon for (A, B) where A: SerRon,
B: SerRon {
    fn ser_ron(&self, d: usize, s: &mut SerRonState) {
        s.out.push('(');
        self.0.ser_ron(d, s);
        s.out.push_str(", ");
        self.1.ser_ron(d, s);
        s.out.push(')');
    }
}

impl<A, B> DeRon for (A, B) where A: DeRon,
B: DeRon {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<(A, B), DeRonErr> {
        s.paren_open(i) ?;
        let r = (de_ron_comma_paren(s, i) ?, de_ron_comma_paren(s, i) ?);
        s.paren_close(i) ?;
        Ok(r)
    }
}

impl<A, B, C> SerRon for (A, B, C) where A: SerRon,
B: SerRon,
C: SerRon {
    fn ser_ron(&self, d: usize, s: &mut SerRonState) {
        s.out.push('(');
        self.0.ser_ron(d, s);
        s.out.push_str(", ");
        self.1.ser_ron(d, s);
        s.out.push_str(", ");
        self.2.ser_ron(d, s);
        s.out.push(')');
    }
}

impl<A, B, C> DeRon for (A, B, C) where A: DeRon,
B: DeRon,
C: DeRon {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<(A, B, C), DeRonErr> {
        s.paren_open(i) ?;
        let r = (de_ron_comma_paren(s, i) ?, de_ron_comma_paren(s, i) ?, de_ron_comma_paren(s, i) ?);
        s.paren_close(i) ?;
        Ok(r)
    }
}

impl<A, B, C, D> SerRon for (A, B, C, D) where A: SerRon,
B: SerRon,
C: SerRon,
D: SerRon {
    fn ser_ron(&self, d: usize, s: &mut SerRonState) {
        s.out.push('(');
        self.0.ser_ron(d, s);
        s.out.push_str(", ");
        self.1.ser_ron(d, s);
        s.out.push_str(", ");
        self.2.ser_ron(d, s);
        s.out.push_str(", ");
        self.3.ser_ron(d, s);
        s.out.push(')');
    }
}

impl<A, B, C, D> DeRon for (A, B, C, D) where A: DeRon,
B: DeRon,
C: DeRon,
D: DeRon {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<(A, B, C, D), DeRonErr> {
        s.paren_open(i) ?;
        let r = (de_ron_comma_paren(s, i) ?, de_ron_comma_paren(s, i) ?, de_ron_comma_paren(s, i) ?, de_ron_comma_paren(s, i) ?);
        s.paren_close(i) ?;
        Ok(r)
    }
}

impl<K, V> SerRon for HashMap<K, V> where K: SerRon,
V: SerRon {
    fn ser_ron(&self, d: usize, s: &mut SerRonState) {
        s.out.push_str("{\n");
        for (k, v) in self {
            s.indent(d + 1);
            k.ser_ron(d + 1, s);
            s.out.push_str(":");
            v.ser_ron(d + 1, s);
            s.conl();
        }
        s.indent(d);
        s.out.push('}');
    }
}

impl<K, V> DeRon for HashMap<K, V> where K: DeRon + Eq + Hash,
V: DeRon {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
    DeRonErr> {
        let mut h = HashMap::new();
        s.curly_open(i) ?;
        while s.tok != DeRonTok::CurlyClose {
            let k = DeRon::de_ron(s, i) ?;
            s.colon(i) ?;
            let v = DeRon::de_ron(s, i) ?;
            s.eat_comma_curly(i) ?;
            h.insert(k, v);
        }
        s.curly_close(i) ?;
        Ok(h)
    }
}

impl<T> SerRon for Box<T> where T: SerRon {
    fn ser_ron(&self, d: usize, s: &mut SerRonState) {
        (**self).ser_ron(d, s)
    }
}

impl<T> DeRon for Box<T> where T: DeRon {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Box<T>, DeRonErr> {
        Ok(Box::new(DeRon::de_ron(s, i) ?))
    }
}