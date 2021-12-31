use std::collections::{HashMap};
use std::hash::Hash;
use std::str::Chars;

pub struct SerJsonState {
    pub out: String
}

impl SerJsonState {
    pub fn indent(&mut self, _d: usize) {
        //for _ in 0..d {
        //    self.out.push_str("    ");
        //}
    }
    
    pub fn field(&mut self, d: usize, field: &str) {
        self.indent(d);
        self.out.push('"');
        self.out.push_str(field);
        self.out.push('"');
        self.out.push(':');
    }
    
    pub fn label(&mut self, label:&str){
        self.out.push('"');
        self.out.push_str(label);
        self.out.push('"');
    }
    
    pub fn conl(&mut self) {
        self.out.push(',')
    }
    
    pub fn st_pre(&mut self) {
        self.out.push('{');
    }
    
    pub fn st_post(&mut self, d: usize) {
        self.indent(d);
        self.out.push('}');
    }
    
}

pub trait SerJson {
    
    fn serialize_json(&self) -> String {
        let mut s = SerJsonState {
            out: String::new()
        };
        self.ser_json(0, &mut s);
        s.out
    }
    
    fn ser_json(&self, d: usize, s: &mut SerJsonState);
}

pub trait DeJson: Sized {
    
    fn deserialize_json(input: &str) -> Result<Self,
    DeJsonErr> {
        let mut state = DeJsonState::default();
        let mut chars = input.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars) ?;
        DeJson::de_json(&mut state, &mut chars)
    }
    
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self,
    DeJsonErr>;
}

#[derive(PartialEq, Debug)]
pub enum DeJsonTok {
    Str,
    Char(char),
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    BareIdent,
    Null,
    Colon,
    CurlyOpen,
    CurlyClose,
    BlockOpen,
    BlockClose,
    Comma,
    Bof,
    Eof
}

impl Default for DeJsonTok {
    fn default() -> Self {DeJsonTok::Bof}
}

#[derive(Default)]
pub struct DeJsonState {
    pub cur: char,
    pub tok: DeJsonTok,
    pub strbuf:String,
    pub numbuf:String,
    pub identbuf:String,
    pub line: usize,
    pub col: usize
}

pub struct DeJsonErr{
    pub msg:String,
    pub line:usize,
    pub col:usize
}

impl std::fmt::Debug for DeJsonErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Json Deserialize error: {}, line:{} col:{}", self.msg, self.line+1, self.col+1)
    }
}

impl DeJsonState {
    pub fn next(&mut self, i: &mut Chars) {
        if let Some(c) = i.next() {
            self.cur = c;
            if self.cur == '\n'{
                self.line += 1;
                self.col = 0;
            }
            else{
                self.col += 1;
            }
        }
        else {
            self.cur = '\0';
        }
    }
    
    pub fn err_exp(&self, name: &str) -> DeJsonErr {
        DeJsonErr{msg:format!("Unexpected key {}", name), line:self.line, col:self.col}
    }
    
    pub fn err_nf(&self, name: &str) -> DeJsonErr {
        DeJsonErr{msg:format!("Key not found {}", name), line:self.line, col:self.col}
    }

    pub fn err_enum(&self, name: &str) -> DeJsonErr {
        DeJsonErr{msg:format!("Enum not defined {}", name), line:self.line, col:self.col}
    }

    pub fn err_token(&self, what:&str) -> DeJsonErr {
        DeJsonErr{msg:format!("Unexpected token {:?} expected {} ", self.tok, what), line:self.line, col:self.col}
    }

    pub fn err_range(&self, what:&str) -> DeJsonErr {
        DeJsonErr{msg:format!("Value out of range {} ", what), line:self.line, col:self.col}
    }

    pub fn err_type(&self, what:&str) -> DeJsonErr {
        DeJsonErr{msg:format!("Token wrong type {} ", what), line:self.line, col:self.col}
    }

    pub fn err_parse(&self, what:&str) -> DeJsonErr {
        DeJsonErr{msg:format!("Cannot parse {} ", what), line:self.line, col:self.col}
    }
    
    pub fn eat_comma_block(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match self.tok {
            DeJsonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeJsonTok::BlockClose => {
                Ok(())
            }
            _ => {
                Err(self.err_token(", or ]"))
            }
        }
    }
    
    pub fn eat_comma_curly(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match self.tok {
            DeJsonTok::Comma => {
                self.next_tok(i) ?;
                Ok(())
            },
            DeJsonTok::CurlyClose => {
                Ok(())
            }
            _ => {
                Err(self.err_token(", or }"))
            }
        }
    }
    
    pub fn colon(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match self.tok {
            DeJsonTok::Colon => {
                self.next_tok(i) ?;
                Ok(())
            },
            _ => {
                Err(self.err_token(":"))
            }
        }
    }
    
    
    pub fn string(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match &mut self.tok {
            DeJsonTok::Str => {
                self.next_tok(i) ?;
                Ok(())
            },
            _ => {
                Err(self.err_token("String"))
            }
        }
    }
    
    pub fn next_colon(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        self.next_tok(i) ?;
        self.colon(i) ?;
        Ok(())
    }
    
    pub fn next_str(&mut self) -> Option<()> {
        if let DeJsonTok::Str = &mut self.tok {
            //let mut s = String::new();
            //std::mem::swap(&mut s, name);
            Some(())
        }
        else {
            None
        }
    }
    
    pub fn block_open(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        if self.tok == DeJsonTok::BlockOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("["))
    }
    
    
    pub fn block_close(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        if self.tok == DeJsonTok::BlockClose {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("]"))
    }
    
    pub fn curly_open(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        if self.tok == DeJsonTok::CurlyOpen {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("{"))
    }
    
    
    pub fn curly_close(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        if self.tok == DeJsonTok::CurlyClose {
            self.next_tok(i) ?;
            return Ok(())
        }
        Err(self.err_token("}"))
    }
    
    pub fn u64_range(&mut self, max: u64) -> Result<u64, DeJsonErr> {
        if let DeJsonTok::U64(value) = self.tok {
            if value > max {
                return Err(self.err_range(&format!("{}>{}", value, max)))
            }
            return Ok(value)
        }
        Err(self.err_token("unsigned integer"))
    }
    
    pub fn i64_range(&mut self, min: i64, max: i64) -> Result<i64, DeJsonErr> {
        if let DeJsonTok::I64(value) = self.tok {
            if value< min {
                return Err(self.err_range(&format!("{}<{}", value, min)))
            }
            return Ok(value)
        }
        if let DeJsonTok::U64(value) = self.tok {
            if value as i64 > max {
                return Err(self.err_range(&format!("{}>{}", value, max)))
            }
            return Ok(value as i64)
        }
        Err(self.err_token("signed integer"))
    }
    
    pub fn as_f64(&mut self) -> Result<f64, DeJsonErr> {
        if let DeJsonTok::I64(value) = self.tok {
            return Ok(value as f64)
        }
        if let DeJsonTok::U64(value) = self.tok {
            return Ok(value as f64)
        }
        if let DeJsonTok::F64(value) = self.tok {
            return Ok(value)
        }
        Err(self.err_token("floating point"))
    }
    
    pub fn as_bool(&mut self) -> Result<bool, DeJsonErr> {
        if let DeJsonTok::Bool(value) = self.tok {
            return Ok(value)
        }
        Err(self.err_token("boolean"))
    }
    
    pub fn as_string(&mut self) -> Result<String, DeJsonErr> {
        if let DeJsonTok::Str = &mut self.tok {
            let mut val = String::new();
            std::mem::swap(&mut val, &mut self.strbuf);
            return Ok(val)
        }
        Err(self.err_token("string"))
    }
    
    pub fn next_tok(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        while self.cur == '\n' || self.cur == '\r' || self.cur == '\t' || self.cur == ' ' {
            self.next(i);
        }
        if self.cur == '\0' {
            self.tok = DeJsonTok::Eof;
            return Ok(())
        }
        match self.cur {
            ':' => {
                self.next(i);
                self.tok = DeJsonTok::Colon;
                return Ok(())
            }
            ',' => {
                self.next(i);
                self.tok = DeJsonTok::Comma;
                return Ok(())
            }
            '[' => {
                self.next(i);
                self.tok = DeJsonTok::BlockOpen;
                return Ok(())
            }
            ']' => {
                self.next(i);
                self.tok = DeJsonTok::BlockClose;
                return Ok(())
            }
            '{' => {
                self.next(i);
                self.tok = DeJsonTok::CurlyOpen;
                return Ok(())
            }
            '}' => {
                self.next(i);
                self.tok = DeJsonTok::CurlyClose;
                return Ok(())
            }
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
                        self.tok = DeJsonTok::F64(num);
                        return Ok(())
                    }
                    else {
                        return Err(self.err_parse("number"));
                    }
                }
                else {
                    if is_neg {
                        if let Ok(num) = self.numbuf.parse() {
                            self.tok = DeJsonTok::I64(num);
                            return Ok(())
                        }
                        else {
                            return Err(self.err_parse("number"));
                        }
                    }
                    if let Ok(num) = self.numbuf.parse() {
                        self.tok = DeJsonTok::U64(num);
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
                    self.tok = DeJsonTok::Bool(true);
                    return Ok(())
                }
                if self.identbuf == "false" {
                    self.tok = DeJsonTok::Bool(false);
                    return Ok(())
                }
                if self.identbuf == "null" {
                    self.tok = DeJsonTok::Null;
                    return Ok(())
                }
                self.tok = DeJsonTok::BareIdent;
                return Err(self.err_token(&format!("Got ##{}## needed true, false, null", self.identbuf)));
            }
            '"' => {
                self.strbuf.clear();
                self.next(i);
                while self.cur != '"' {
                    if self.cur == '\\' {
                        self.next(i);
                        match self.cur{
                            'n'=>self.strbuf.push('\n'),
                            'r'=>self.strbuf.push('\r'),
                            't'=>self.strbuf.push('\t'),
                            '0'=>self.strbuf.push('\0'),
                            '\0'=>{
                                return Err(self.err_parse("string"));
                            },
                            _=>self.strbuf.push(self.cur)
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
                self.tok = DeJsonTok::Str;
                return Ok(())
            },
            _ => {
                return Err(self.err_token("tokenizer"));
            }
        }
    }
}

macro_rules!impl_ser_de_json_unsigned {
    ( $ ty: ident, $ max: expr) => {
        impl SerJson for $ ty {
            fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
                s.out.push_str(&self.to_string());
            }
        }
        
        impl DeJson for $ ty {
            fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result< $ ty,
            DeJsonErr> {
                let val = s.u64_range( $ max as u64) ?;
                s.next_tok(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

macro_rules!impl_ser_de_json_signed {
    ( $ ty: ident, $ min: expr, $ max: expr) => {
        impl SerJson for $ ty {
            fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
                s.out.push_str(&self.to_string());
            }
        }
        
        impl DeJson for $ ty {
            fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result< $ ty,
            DeJsonErr> {
                //s.is_prefix(p, i) ?;
                let val = s.i64_range( $ min as i64, $ max as i64) ?;
                s.next_tok(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

macro_rules!impl_ser_de_json_float {
    ( $ ty: ident) => {
        impl SerJson for $ ty {
            fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
                s.out.push_str(&self.to_string());
            }
        }
        
        impl DeJson for $ ty {
            fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result< $ ty,
            DeJsonErr> {
                //s.is_prefix(p, i) ?;
                let val = s.as_f64() ?;
                s.next_tok(i) ?;
                return Ok(val as $ ty);
            }
        }
    }
}

impl_ser_de_json_unsigned!(usize, std::u64::MAX);
impl_ser_de_json_unsigned!(u64, std::u64::MAX);
impl_ser_de_json_unsigned!(u32, std::u32::MAX);
impl_ser_de_json_unsigned!(u16, std::u16::MAX);
impl_ser_de_json_unsigned!(u8, std::u8::MAX);
impl_ser_de_json_signed!(i64, std::i64::MIN, std::i64::MAX);
impl_ser_de_json_signed!(i32, std::i64::MIN, std::i64::MAX);
impl_ser_de_json_signed!(i16, std::i64::MIN, std::i64::MAX);
impl_ser_de_json_signed!(i8, std::i64::MIN, std::i8::MAX);
impl_ser_de_json_float!(f64);
impl_ser_de_json_float!(f32);

impl<T> SerJson for Option<T> where T: SerJson {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        if let Some(v) = self {
            v.ser_json(d, s);
        }
        else {
            s.out.push_str("None");
        }
    }
}

impl<T> DeJson for Option<T> where T: DeJson{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self,
    DeJsonErr> {
        if let DeJsonTok::Null = s.tok {
            s.next_tok(i) ?;
            return Ok(None)
        }
        Ok(Some(DeJson::de_json(s, i) ?))
    }
}

impl SerJson for bool {
    fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
        if *self {
            s.out.push_str("true")
        }
        else {
            s.out.push_str("false")
        }
    }
}

impl DeJson for bool {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<bool, DeJsonErr> {
        let val = s.as_bool() ?;
        s.next_tok(i) ?;
        return Ok(val);
    }
}

impl SerJson for String {
    fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
        s.out.push('"');
        for c in self.chars() {
            match c{
                '\n'=>{s.out.push('\\');s.out.push('n');},
                '\r'=>{s.out.push('\\');s.out.push('r');},
                '\t'=>{s.out.push('\\');s.out.push('t');},
                '\0'=>{s.out.push('\\');s.out.push('0');},
                '\\'=>{s.out.push('\\');s.out.push('\\');},
                '"'=>{s.out.push('\\');s.out.push('"');},
                _=>s.out.push(c)
            }
        }
        s.out.push('"');
    }
}

impl DeJson for String {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<String, DeJsonErr> {
        let val = s.as_string() ?;
        s.next_tok(i) ?;
        return Ok(val);
    }
}

impl<T> SerJson for Vec<T> where T: SerJson {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('[');
        if self.len() > 0{
            let last = self.len() -1;
            for (index,item) in self.iter().enumerate() {
                s.indent(d + 1);
                item.ser_json(d + 1, s);
                if index != last{
                    s.out.push(',');
                }
            }
        }
        s.out.push(']');
    }
}

impl<T> DeJson for Vec<T> where T: DeJson {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Vec<T>, DeJsonErr> {
        let mut out = Vec::new();
        s.block_open(i) ?;
        
        while s.tok != DeJsonTok::BlockClose {
            out.push(DeJson::de_json(s, i) ?);
            s.eat_comma_block(i) ?;
        }
        s.block_close(i) ?;
        Ok(out)
    }
}

impl<T> SerJson for [T] where T: SerJson {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('[');
        let last = self.len() -1;
        for (index,item) in self.iter().enumerate() {
            item.ser_json(d + 1, s);
            if index != last{
                s.out.push(',');
            }
        }
        s.out.push(']');
    }
}

unsafe fn de_json_array_impl_inner<T>(top: *mut T, count: usize, s: &mut DeJsonState, i: &mut Chars) -> Result<(), DeJsonErr> where T:DeJson{
    s.block_open(i) ?;
    for c in 0..count {
        top.add(c).write(DeJson::de_json(s, i) ?);
        s.eat_comma_block(i) ?;
    }
    s.block_close(i) ?;
    Ok(())
}

macro_rules!de_json_array_impl {
    ( $($count:expr),*) => {
        $(
        impl<T> DeJson for [T; $count] where T: DeJson {
            fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self,
            DeJsonErr> {
                unsafe{
                    let mut to = std::mem::MaybeUninit::<[T; $count]>::uninit();
                    let top: *mut T = std::mem::transmute(&mut to);
                    de_json_array_impl_inner(top, $count, s, i)?;
                    Ok(to.assume_init())
                }
            }
        }
        )*
    }
}

de_json_array_impl!(2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32);

fn de_json_comma_block<T>(s: &mut DeJsonState, i: &mut Chars) -> Result<T, DeJsonErr> where T: DeJson {
    let t = DeJson::de_json(s, i);
    s.eat_comma_block(i) ?;
    t
}

impl<A, B> SerJson for (A, B) where A: SerJson,
B: SerJson {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('[');
        self.0.ser_json(d, s);
        s.out.push(',');
        self.1.ser_json(d, s);
        s.out.push(']');
    }
}

impl<A, B> DeJson for (A, B) where A: DeJson,
B: DeJson {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<(A, B), DeJsonErr> {
        s.block_open(i) ?;
        let r = (de_json_comma_block(s, i) ?, de_json_comma_block(s, i) ?);
        s.block_close(i) ?;
        Ok(r)
    }
}

impl<A, B, C> SerJson for (A, B, C) where A: SerJson,
B: SerJson,
C: SerJson {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('[');
        self.0.ser_json(d, s);
        s.out.push(',');
        self.1.ser_json(d, s);
        s.out.push(',');
        self.2.ser_json(d, s);
        s.out.push(']');
    }
}

impl<A, B, C> DeJson for (A, B, C) where A: DeJson,
B: DeJson,
C: DeJson {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<(A, B, C), DeJsonErr> {
        s.block_open(i) ?;
        let r = (de_json_comma_block(s, i) ?, de_json_comma_block(s, i) ?, de_json_comma_block(s, i) ?);
        s.block_close(i) ?;
        Ok(r)
    }
}

impl<A, B, C, D> SerJson for (A, B, C, D) where A: SerJson,
B: SerJson,
C: SerJson,
D: SerJson {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('[');
        self.0.ser_json(d, s);
        s.out.push(',');
        self.1.ser_json(d, s);
        s.out.push(',');
        self.2.ser_json(d, s);
        s.out.push(',');
        self.3.ser_json(d, s);
        s.out.push(']');
    }
}

impl<A, B, C, D> DeJson for (A, B, C, D) where A: DeJson,
B: DeJson,
C: DeJson,
D: DeJson {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<(A, B, C, D), DeJsonErr> {
        s.block_open(i) ?;
        let r = (de_json_comma_block(s, i) ?, de_json_comma_block(s, i) ?, de_json_comma_block(s, i) ?, de_json_comma_block(s, i) ?);
        s.block_close(i) ?;
        Ok(r)
    }
}

impl<K, V> SerJson for HashMap<K, V> where K: SerJson,
V: SerJson {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('{');
        let last = self.len() - 1;
        let mut index = 0;
        for (k, v) in self {
            s.indent(d + 1);
            k.ser_json(d + 1, s);
            s.out.push(':');
            v.ser_json(d + 1, s);
            if index != last{
                s.conl();
            }
            index += 1;
        }
        s.indent(d);
        s.out.push('}');
    }
}

impl<K, V> DeJson for HashMap<K, V> where K: DeJson + Eq + Hash,
V: DeJson  {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self,
    DeJsonErr> {
        let mut h = HashMap::new();
        s.curly_open(i) ?;
        while s.tok != DeJsonTok::CurlyClose {
            let k = DeJson::de_json(s, i) ?;
            s.colon(i) ?;
            let v = DeJson::de_json(s, i) ?;
            s.eat_comma_curly(i) ?;
            h.insert(k, v);
        }
        s.curly_close(i) ?;
        Ok(h)
    }
}

impl<T> SerJson for Box<T> where T: SerJson {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        (**self).ser_json(d, s)
    }
}

impl<T> DeJson for Box<T> where T: DeJson {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Box<T>, DeJsonErr> {
        Ok(Box::new(DeJson::de_json(s, i) ?))
    }
}