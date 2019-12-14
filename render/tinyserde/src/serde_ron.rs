//use std::collections::{HashMap};
//use std::hash::Hash;
use std::str::Chars;

pub fn ser_ron_indent(d: usize, s: &mut String) {
    for _ in 0..d {
        s.push_str("    ");
    }
}

pub fn ser_ron_pref(d: usize, p: &str, s: &mut String) {
    if p.len()>0 {
        ser_ron_indent(d, s);
        s.push_str(p);
        s.push_str(":");
    }
}

pub fn ser_ron_struct_pre(d: usize, p: &str, s: &mut String) {
    ser_ron_pref(d, p, s);
    s.push_str("(\n");
}

pub fn ser_ron_struct_post(d: usize, p: &str, s: &mut String) {
    ser_ron_indent(d, s);
    s.push_str(")");
    ser_ron_post(p, s);
}

pub fn ser_ron_post(p: &str, s: &mut String) {
    if p.len()>0 {
        s.push_str(",\n");
    }
}

pub trait SerRon {
    
    fn serialize_ron(&self) -> String {
        let mut output = String::new();
        self.ser_ron(0, "", &mut output);
        output
    }
    
    fn ser_ron(&self, d: usize, p: &str, s: &mut String);
}


pub trait DeRon: Sized {
    
    fn deserialize_ron(input: &str) -> Result<Self,
    String> {
        let mut state = DeRonState::default();
        let mut chars = input.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars) ?;
        DeRon::de_ron("", &mut state, &mut chars)
    }
    
    fn de_ron(p: &str, s: &mut DeRonState, i: &mut Chars) -> Result<Self,
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
    
    pub fn is_prefix_paren_open(&mut self, pref: &str, i: &mut Chars) -> Result<(), String> {
        self.is_prefix(pref, i) ?;
        self.is_paren_open(i) ?;
        Ok(())
    }
    
    pub fn is_prefix(&mut self, pref: &str, i: &mut Chars) -> Result<(), String> {
        if pref.len() == 0 {
            return Ok(())
        }
        if let DeRonTok::Ident(name) = &self.tok {
            if name != pref {
                return Err(format!("ron object property expected {} got {}", name, pref))
            }
        }
        else {
            return Err(format!("expected property {} but found {:?}", pref, self.tok))
        }
        
        self.next_tok(i) ?;
        if let DeRonTok::Colon = self.tok {
            self.next_tok(i) ?;
            Ok(())
        }
        else {
            Err(format!("ron object property not followed by colon (:) {}", pref))
        }
    }
    
    // this thing accepts a comma, or )}]
    pub fn is_tail(&mut self, i: &mut Chars) -> Result<(), String> {
        self.next_tok(i) ?;
        match self.tok {
            DeRonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeRonTok::ParenClose | DeRonTok::CurlyClose | DeRonTok::BlockClose => {
                Ok(())
            }
            _ => {
                Err(format!("Unexpected token {:?}", self.tok))
            }
        }
    }
    
    pub fn is_paren_open(&mut self, i: &mut Chars) -> Result<(), String> {
        if self.tok == DeRonTok::ParenOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(format!("Next token not paren open {:?}", self.tok))
    }
    
    pub fn is_paren_close(&mut self, i: &mut Chars) -> Result<(), String> {
        if self.tok == DeRonTok::ParenClose {
            // could be final token
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(format!("Next token not paren close {:?}", self.tok))
    }
    
    pub fn is_u64_range(&mut self, max: u64) -> Result<u64, String> {
        if let DeRonTok::U64(value) = self.tok {
            if value > max {
                return Err(format!("Value out of range {}>{}", value, max))
            }
            return Ok(value)
        }
        Err(format!("Next token not a integer<{} {:?}", max, self.tok))
    }
    
    pub fn is_i64_range(&mut self, min: i64, max: i64) -> Result<i64, String> {
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
    
    pub fn is_f64(&mut self) -> Result<f64, String> {
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
    
    pub fn is_bool(&mut self) -> Result<bool, String> {
        if let DeRonTok::Bool(value) = self.tok {
            return Ok(value)
        }
        Err(format!("Next token not a boolean {:?}", self.tok))
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
            _ => {
                return Err(format!("Unexpected token {}", self.cur));
            }
        }
    }
}

macro_rules!impl_ser_de_ron_unsigned {
    ( $ ty: ident, $ max: expr) => {
        impl SerRon for $ ty {
            fn ser_ron(&self, d: usize, p: &str, s: &mut String) {
                ser_ron_pref(d, p, s);
                s.push_str(&self.to_string());
                ser_ron_post(p, s);
            }
        }
        
        impl DeRon for $ ty {
            fn de_ron(p: &str, s: &mut DeRonState, i: &mut Chars) -> Result< $ ty,
            String> {
                s.is_prefix(p, i) ?;
                let val = s.is_u64_range( $ max as u64) ?;
                s.is_tail(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

macro_rules!impl_ser_de_ron_signed {
    ( $ ty: ident, $ min: expr, $ max: expr) => {
        impl SerRon for $ ty {
            fn ser_ron(&self, d: usize, p: &str, s: &mut String) {
                ser_ron_pref(d, p, s);
                s.push_str(&self.to_string());
                ser_ron_post(p, s);
            }
        }
        
        impl DeRon for $ ty {
            fn de_ron(p: &str, s: &mut DeRonState, i: &mut Chars) -> Result< $ ty,
            String> {
                s.is_prefix(p, i) ?;
                let val = s.is_i64_range( $ min as i64, $ max as i64) ?;
                s.is_tail(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

macro_rules!impl_ser_de_ron_float {
    ( $ ty: ident) => {
        impl SerRon for $ ty {
            fn ser_ron(&self, d: usize, p: &str, s: &mut String) {
                ser_ron_pref(d, p, s);
                s.push_str(&self.to_string());
                ser_ron_post(p, s);
            }
        }
        
        impl DeRon for $ ty {
            fn de_ron(p: &str, s: &mut DeRonState, i: &mut Chars) -> Result< $ ty,
            String> {
                s.is_prefix(p, i) ?;
                let val = s.is_f64() ?;
                s.is_tail(i) ?;
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
    fn ser_ron(&self, d: usize, p: &str, s: &mut String) {
        if let Some(v) = self {
            v.ser_ron(d, p, s);
        }
        // skip it entirely. in Ron optional values are omitted
    }
}

impl<T> DeRon for Option<T> where T: DeRon + std::fmt::Debug {
    fn de_ron(p: &str, s: &mut DeRonState, i: &mut Chars) -> Result<Self,
    String> {
        if let DeRonTok::Ident(name) = &s.tok {
            if name != p {
                return Ok(None);
            }
            else {
                s.next_tok(i) ?;
                if let DeRonTok::Colon = s.tok {
                    s.next_tok(i) ?;
                    if let DeRonTok::Ident(name) = &s.tok {
                        if name == "None" { // its a None
                            s.next_tok(i) ?;
                            return Ok(None);
                        }
                    }
                    let x: T = DeRon::de_ron("", s, i) ?;
                    Ok(Some(x))
                }
                else {
                    Err(format!("ron object property not followed by colon (:) {}", p))
                }
            }
        }
        else {
            return Ok(None);
        }
    }
}

impl SerRon for bool {
    fn ser_ron(&self, d: usize, p: &str, s: &mut String) {
        ser_ron_pref(d, p, s);
        if *self {
            s.push_str("true")
        }
        else {
            s.push_str("false")
        }
        ser_ron_post(p, s);
    }
}

impl DeRon for bool {
    fn de_ron(p: &str, s: &mut DeRonState, i: &mut Chars) -> Result<bool, String> {
        s.is_prefix(p, i) ?;
        let val = s.is_bool() ?;
        s.is_tail(i) ?;
        return Ok(val);
    }
}

/*

impl SerBin for String {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let len = self.len();
        len.ser_bin(s);
        s.extend_from_slice(self.as_bytes());
    }
}

impl DeBin for String {
    fn de_bin(o:&mut usize, d:&[u8])->String {
        let len:usize = DeBin::de_bin(o,d);
        let r = std::str::from_utf8(&d[*o..(*o+len)]).unwrap().to_string();
        *o += len;
        r
    }
}

impl<T> SerBin for Vec<T> where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let len = self.len();
        len.ser_bin(s);
        for item in self {
            item.ser_bin(s);
        }
    }
}

impl<T> DeBin for Vec<T> where T:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Vec<T> {
        let len:usize = DeBin::de_bin(o,d);
        let mut out = Vec::new();
        for _ in 0..len{
            out.push(DeBin::de_bin(o,d))
        }
        out
    }
}

impl<T> SerBin for Option<T> where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        if let Some(v) = self{
            s.push(1);
            v.ser_bin(s);
        }
        else{
            s.push(0);
        }
    }
}

impl<T> DeBin for Option<T> where T:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Option<T> {
        let m = d[*o];
        *o += 1;
        if m == 1{
            Some(DeBin::de_bin(o,d))
        }
        else{
            None
        }
    }
}

impl<T> SerBin for [T] where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        for item in self {
            item.ser_bin(s);
        }
    }
}

macro_rules! expand_de_bin {
    ($o:expr, $($d:expr),*) => ([$(DeBin::de_bin($o, $d)),*]);
}


// kinda nasty i have to do this this way, is there a better one?
impl<T> DeBin for [T;2] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d)}}
impl<T> DeBin for [T;3] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d)}}
impl<T> DeBin for [T;4] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d)}}
impl<T> DeBin for [T;5] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d)}}
impl<T> DeBin for [T;6] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d)}}
impl<T> DeBin for [T;7] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;8] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;9] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;10] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;11] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;12] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;13] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;14] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;15] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;16] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;17] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;18] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;19] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;20] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;21] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;22] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;23] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;24] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;25] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;26] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;27] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;28] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;29] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;30] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;31] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;32] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}

impl<A,B> SerBin for (A,B) where A: SerBin, B:SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
    }
}

impl<A,B> DeBin for (A,B) where A:DeBin, B:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->(A,B) {(DeBin::de_bin(o,d),DeBin::de_bin(o,d))}
}

impl<A,B,C> SerBin for (A,B,C) where A: SerBin, B:SerBin, C:SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
        self.2.ser_bin(s);
    }
}

impl<A,B,C> DeBin for (A,B,C) where A:DeBin, B:DeBin, C:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->(A,B,C) {(DeBin::de_bin(o,d),DeBin::de_bin(o,d),DeBin::de_bin(o,d))}
}

impl<A,B,C,D> SerBin for (A,B,C,D) where A: SerBin, B:SerBin, C:SerBin, D:SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
        self.2.ser_bin(s);
        self.3.ser_bin(s);
    }
}

impl<A,B,C,D> DeBin for (A,B,C,D) where A:DeBin, B:DeBin, C:DeBin, D:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->(A,B,C,D) {(DeBin::de_bin(o,d),DeBin::de_bin(o,d),DeBin::de_bin(o,d),DeBin::de_bin(o,d))}
}

impl<K, V> SerBin for HashMap<K, V> where K: SerBin,
V: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let len = self.len();
        len.ser_bin(s);
        for (k, v) in self {
            k.ser_bin(s);
            v.ser_bin(s);
        }
    }
}

impl<K, V> DeBin for HashMap<K, V> where K: DeBin + Eq + Hash,
V: DeBin + Eq {
    fn de_bin(o:&mut usize, d:&[u8])->Self{
        let len:usize = DeBin::de_bin(o,d);
        let mut h = HashMap::new();
        for _ in 0..len{
            let k = DeBin::de_bin(o,d);
            let v = DeBin::de_bin(o,d);
            h.insert(k, v);
        }
        h
    }
}


impl<T> SerBin for Box<T> where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        (**self).ser_bin(s)
    }
}

impl<T> DeBin for Box<T> where T: DeBin {
    fn de_bin(o:&mut usize, d:&[u8])->Box<T> {
        Box::new(DeBin::de_bin(o,d))
    }
}*/