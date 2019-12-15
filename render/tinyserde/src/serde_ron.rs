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
    String> {
        let mut state = DeRonState::default();
        let mut chars = input.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars) ?;
        DeRon::de_ron(&mut state, &mut chars)
    }
    
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
    String>;
}

#[derive(PartialEq, Debug)]
pub enum DeRonTok {
    Ident(String),
    Str(String),
    Char(char),
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
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
    cur: char,
    tok: DeRonTok,
}

impl DeRonState {
    pub fn next(&mut self, i: &mut Chars) {
        if let Some(c) = i.next() {
            self.cur = c;
        }
        else {
            self.cur = '\0';
        }
    }
    
    pub fn unexp(&self, name: &str) -> Result<(), String> {
        Err(format!("Unexpected key {}", name))
    }
    
    pub fn nf(&self, name: &str) -> String {
        format!("Key not defined {}", name)
    }
    
    pub fn invenum(&self, name: &str) -> String {
        format!("Enum option not defined {}", name)
    }
    
    pub fn eat_comma_paren(&mut self, i: &mut Chars) -> Result<(), String> {
        match self.tok {
            DeRonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeRonTok::ParenClose => {
                Ok(())
            }
            _ => {
                Err(format!("Unexpected token {:?}", self.tok))
            }
        }
    }
    
    pub fn eat_comma_block(&mut self, i: &mut Chars) -> Result<(), String> {
        match self.tok {
            DeRonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeRonTok::BlockClose => {
                Ok(())
            }
            _ => {
                Err(format!("Unexpected token {:?}", self.tok))
            }
        }
    }
    
    pub fn eat_comma_curly(&mut self, i: &mut Chars) -> Result<(), String> {
        match self.tok {
            DeRonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeRonTok::CurlyClose => {
                Ok(())
            }
            _ => {
                Err(format!("Unexpected token {:?}", self.tok))
            }
        }
    }
    
    pub fn colon(&mut self, i: &mut Chars) -> Result<(), String> {
        match self.tok {
            DeRonTok::Colon => {
                self.next_tok(i) ?;
                Ok(())
            },
            _ => {
                Err(format!("Unexpected token {:?}", self.tok))
            }
        }
    }
    
    pub fn ident(&mut self, i: &mut Chars) -> Result<String, String> {
        match &mut self.tok {
            DeRonTok::Ident(val) => {
                let mut ret = String::new();
                std::mem::swap(val, &mut ret);
                self.next_tok(i) ?;
                Ok(ret)
            },
            _ => {
                Err(format!("Unexpected token {:?}", self.tok))
            }
        }
    }
    
    pub fn next_colon(&mut self, i: &mut Chars) -> Result<(), String> {
        self.next_tok(i) ?;
        self.colon(i) ?;
        Ok(())
    }
    
    pub fn next_ident(&mut self) -> Option<String> {
        if let DeRonTok::Ident(name) = &mut self.tok {
            let mut s = String::new();
            std::mem::swap(&mut s, name);
            Some(s)
        }
        else {
            None
        }
    }
    
    pub fn paren_open(&mut self, i: &mut Chars) -> Result<(), String> {
        if self.tok == DeRonTok::ParenOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(format!("Next token not paren open {:?}", self.tok))
    }
    
    
    pub fn paren_close(&mut self, i: &mut Chars) -> Result<(), String> {
        if self.tok == DeRonTok::ParenClose {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(format!("Next token not paren close {:?}", self.tok))
    }
    
    pub fn block_open(&mut self, i: &mut Chars) -> Result<(), String> {
        if self.tok == DeRonTok::BlockOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(format!("Next token not block open {:?}", self.tok))
    }
    
    
    pub fn block_close(&mut self, i: &mut Chars) -> Result<(), String> {
        if self.tok == DeRonTok::BlockClose {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(format!("Next token not block close {:?}", self.tok))
    }
    
    pub fn curly_open(&mut self, i: &mut Chars) -> Result<(), String> {
        if self.tok == DeRonTok::CurlyOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(format!("Next token not curly open {:?}", self.tok))
    }
    
    
    pub fn curly_close(&mut self, i: &mut Chars) -> Result<(), String> {
        if self.tok == DeRonTok::CurlyClose {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(format!("Next token not curly close {:?}", self.tok))
    }
    
    
    pub fn u64_range(&mut self, max: u64) -> Result<u64, String> {
        if let DeRonTok::U64(value) = self.tok {
            if value > max {
                return Err(format!("Value out of range {}>{}", value, max))
            }
            return Ok(value)
        }
        Err(format!("Next token not a integer<{} {:?}", max, self.tok))
    }
    
    pub fn i64_range(&mut self, min: i64, max: i64) -> Result<i64, String> {
        if let DeRonTok::I64(value) = self.tok {
            if value< min {
                return Err(format!("Value out of range {}<{}", value, min))
            }
            return Ok(value)
        }
        if let DeRonTok::U64(value) = self.tok {
            if value as i64 > max {
                return Err(format!("Value out of range {}>{}", value, max))
            }
            return Ok(value as i64)
        }
        Err(format!("Next token not a signed integer {:?}", self.tok))
    }
    
    pub fn as_f64(&mut self) -> Result<f64, String> {
        if let DeRonTok::I64(value) = self.tok {
            return Ok(value as f64)
        }
        if let DeRonTok::U64(value) = self.tok {
            return Ok(value as f64)
        }
        if let DeRonTok::F64(value) = self.tok {
            return Ok(value)
        }
        Err(format!("Next token not a number {:?}", self.tok))
    }
    
    pub fn as_bool(&mut self) -> Result<bool, String> {
        if let DeRonTok::Bool(value) = self.tok {
            return Ok(value)
        }
        Err(format!("Next token not a boolean {:?}", self.tok))
    }
    
    pub fn as_string(&mut self) -> Result<String, String> {
        if let DeRonTok::Str(value) = &mut self.tok {
            let mut val = String::new();
            std::mem::swap(&mut val, value);
            return Ok(val)
        }
        Err(format!("Next token not a string {:?}", self.tok))
    }
    
    pub fn next_tok(&mut self, i: &mut Chars) -> Result<(), String> {
        while self.cur == '\n' || self.cur == '\r' || self.cur == '\t' || self.cur == ' ' {
            self.next(i);
        }
        if self.cur == '\0' {
            self.tok = DeRonTok::Eof;
            return Ok(())
        }
        match self.cur {
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
            '-' | '0'..='9' => {
                let mut num = String::new();
                let is_neg = if self.cur == '-' {
                    num.push(self.cur);
                    self.next(i);
                    true
                }
                else {
                    false
                };
                while self.cur >= '0' && self.cur <= '9' {
                    num.push(self.cur);
                    self.next(i);
                }
                if self.cur == '.' {
                    num.push(self.cur);
                    self.next(i);
                    while self.cur >= '0' && self.cur <= '9' {
                        num.push(self.cur);
                        self.next(i);
                    }
                    if let Ok(num) = num.parse() {
                        self.tok = DeRonTok::F64(num);
                        return Ok(())
                    }
                    else {
                        return Err(format!("cannot parse number {}", num));
                    }
                }
                else {
                    if is_neg {
                        if let Ok(num) = num.parse() {
                            self.tok = DeRonTok::I64(num);
                            return Ok(())
                        }
                        else {
                            return Err(format!("cannot parse number {}", num));
                        }
                    }
                    if let Ok(num) = num.parse() {
                        self.tok = DeRonTok::U64(num);
                        return Ok(())
                    }
                    else {
                        return Err(format!("cannot parse number {}", num));
                    }
                }
            },
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut ident = String::new();
                while self.cur >= 'a' && self.cur <= 'z'
                    || self.cur >= 'A' && self.cur <= 'Z'
                    || self.cur == '_' {
                    ident.push(self.cur);
                    self.next(i);
                }
                if ident == "true" {
                    self.tok = DeRonTok::Bool(true);
                    return Ok(())
                }
                if ident == "false" {
                    self.tok = DeRonTok::Bool(false);
                    return Ok(())
                }
                self.tok = DeRonTok::Ident(ident);
                return Ok(())
            },
            '"' => {
                let mut val = String::new();
                self.next(i);
                while self.cur != '"' {
                    if self.cur == '\\' {
                        self.next(i);
                    }
                    if self.cur == '\0' {
                        return Err(format!("Unexpected end of string"));
                    }
                    val.push(self.cur);
                    self.next(i);
                }
                self.next(i);
                self.tok = DeRonTok::Str(val);
                return Ok(())
            },
            _ => {
                return Err(format!("Unexpected token {}", self.cur));
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
            String> {
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
            String> {
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
            String> {
                //s.is_prefix(p, i) ?;
                let val = s.as_f64() ?;
                s.next_tok(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

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

impl<T> DeRon for Option<T> where T: DeRon + std::fmt::Debug {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
    String> {
        if let DeRonTok::Ident(name) = &s.tok {
            if name == "None" {
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
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<bool, String> {
        let val = s.as_bool() ?;
        s.next_tok(i) ?;
        return Ok(val);
    }
}

impl SerRon for String {
    fn ser_ron(&self, _d: usize, s: &mut SerRonState) {
        s.out.push('"');
        for c in self.chars() {
            if c == '\\' || c == '"' {
                s.out.push('\\');
            }
            s.out.push(c)
        }
        s.out.push('"');
    }
}

impl DeRon for String {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<String, String> {
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
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Vec<T>, String> {
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
        let last = self.len() -1;
        for (index,item) in self.iter().enumerate() {
            item.ser_ron(d + 1, s);
            if index != last{
                s.out.push_str(", ");
            }
        }
        s.out.push(')');
    }
}

unsafe fn de_ron_array_impl_inner<T>(top: *mut T, count: usize, s: &mut DeRonState, i: &mut Chars) -> Result<(), String> where T:DeRon{
    s.paren_open(i) ?;
    for c in 0..count {
        top.add(c).write(DeRon::de_ron(s, i) ?);
        s.eat_comma_paren(i) ?;
    }
    s.paren_close(i) ?;
    Ok(())
}

macro_rules!de_ron_array_impl {
    ( $($count:expr),*) => {
        $(
        impl<T> DeRon for [T; $count] where T: DeRon {
            fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
            String> {
                unsafe{
                    let mut to = std::mem::MaybeUninit::<[T; $count]>::uninit();
                    let top: *mut T = std::mem::transmute(&mut to);
                    de_ron_array_impl_inner(top, $count, s, i)?;
                    Ok(to.assume_init())
                }
            }
        }
        )*
    }
}

de_ron_array_impl!(2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32);

fn de_ron_comma_paren<T>(s: &mut DeRonState, i: &mut Chars) -> Result<T, String> where T: DeRon {
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
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<(A, B), String> {
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
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<(A, B, C), String> {
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
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<(A, B, C, D), String> {
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
V: DeRon + Eq {
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
    String> {
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
    fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Box<T>, String> {
        Ok(Box::new(DeRon::de_ron(s, i) ?))
    }
}