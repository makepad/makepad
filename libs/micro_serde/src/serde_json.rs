use makepad_live_id::LiveId;
use std::collections::HashMap;
use std::hash::Hash;
use std::str::Chars;

pub struct SerJsonState {
    pub out: String,
    /// Human-readable output: a newline and four spaces per depth before
    /// every field, array item and closing bracket. Off = compact.
    pub pretty: bool,
}

impl SerJsonState {
    pub fn new() -> Self {
        Self { out: String::new(), pretty: false }
    }

    pub fn new_pretty() -> Self {
        Self { out: String::new(), pretty: true }
    }

    pub fn indent(&mut self, d: usize) {
        if self.pretty {
            self.out.push('\n');
            for _ in 0..d {
                self.out.push_str("    ");
            }
        }
    }

    pub fn field(&mut self, d: usize, field: &str) {
        self.indent(d);
        self.out.push('"');
        self.out.push_str(field);
        self.out.push('"');
        self.out.push(':');
        if self.pretty {
            self.out.push(' ');
        }
    }

    pub fn label(&mut self, label: &str) {
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
        let mut s = SerJsonState::new();
        self.ser_json(0, &mut s);
        s.out
    }

    /// Indented, one field or item per line — for files people read.
    fn serialize_json_pretty(&self) -> String {
        let mut s = SerJsonState::new_pretty();
        self.ser_json(0, &mut s);
        s.out
    }

    fn ser_json(&self, d: usize, s: &mut SerJsonState);
}

pub trait DeJson: Sized {
    fn deserialize_json(input: &str) -> Result<Self, DeJsonErr> {
        let mut state = DeJsonState::default();
        let mut chars = input.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars)?;
        DeJson::de_json(&mut state, &mut chars)
    }

    fn deserialize_json_lenient(input: &str) -> Result<Self, DeJsonErr> {
        let mut state = DeJsonState::default();
        state.lenient = true;
        let mut chars = input.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars)?;
        DeJson::de_json(&mut state, &mut chars)
    }

    /// Like `deserialize_json`, but the input must be exactly one JSON
    /// value: anything but whitespace after it is an error, and numbers
    /// with a leading zero are rejected.
    fn deserialize_json_strict(input: &str) -> Result<Self, DeJsonErr> {
        let mut state = DeJsonState::default();
        state.strict = true;
        let mut chars = input.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars)?;
        let value = DeJson::de_json(&mut state, &mut chars)?;
        if state.tok != DeJsonTok::Eof {
            return Err(state.err_msg("Trailing content after the JSON value"));
        }
        Ok(value)
    }

    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self, DeJsonErr>;
}

/// Deepest nesting the parser accepts, the same as serde_json: it keeps a
/// pathological input from turning recursion into a stack overflow.
pub const MAX_JSON_DEPTH: u32 = 128;

#[derive(PartialEq, Debug, Default)]
pub enum DeJsonTok {
    Str,
    Char(char),
    U64(u64),
    U128(u128),
    I64(i64),
    I128(i128),
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
    #[default]
    Bof,
    Eof,
}

#[derive(Default)]
pub struct DeJsonState {
    pub cur: char,
    pub tok: DeJsonTok,
    pub strbuf: String,
    pub numbuf: String,
    pub identbuf: String,
    pub line: usize,
    pub col: usize,
    pub lenient: bool,
    /// Reject leading-zero numbers (`deserialize_json_strict`).
    pub strict: bool,
    /// Open arrays and objects at this point of the parse.
    pub depth: u32,
}

pub struct DeJsonErr {
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Debug for DeJsonErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Json Deserialize error: {}, line:{} col:{}",
            self.msg,
            self.line + 1,
            self.col + 1
        )
    }
}

impl std::fmt::Display for DeJsonErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for DeJsonErr {}

impl DeJsonState {
    pub fn next(&mut self, i: &mut Chars) {
        if let Some(c) = i.next() {
            self.cur = c;
            if self.cur == '\n' {
                self.line += 1;
                self.col = 0;
            } else {
                self.col += 1;
            }
        } else {
            self.cur = '\0';
        }
    }

    /// Four hex digits after the current char; a bad digit counts as 0.
    fn hex4(&mut self, i: &mut Chars) -> u32 {
        let mut a = 0u32;
        for _ in 0..4 {
            self.next(i);
            a = (a << 4) | self.cur.to_digit(16).unwrap_or(0);
        }
        a
    }

    pub fn err_exp(&self, name: &str) -> DeJsonErr {
        DeJsonErr {
            msg: format!("Unexpected key {}", name),
            line: self.line,
            col: self.col,
        }
    }

    pub fn err_msg(&self, msg: &str) -> DeJsonErr {
        DeJsonErr {
            msg: format!("{}", msg),
            line: self.line,
            col: self.col,
        }
    }

    pub fn err_nf(&self, name: &str) -> DeJsonErr {
        DeJsonErr {
            msg: format!("Key not found {}", name),
            line: self.line,
            col: self.col,
        }
    }

    pub fn err_enum(&self, name: &str) -> DeJsonErr {
        DeJsonErr {
            msg: format!("Enum not defined {}", name),
            line: self.line,
            col: self.col,
        }
    }

    pub fn err_token(&self, what: &str) -> DeJsonErr {
        DeJsonErr {
            msg: format!("Unexpected token {:?} expected {} ", self.tok, what),
            line: self.line,
            col: self.col,
        }
    }

    pub fn err_range(&self, what: &str) -> DeJsonErr {
        DeJsonErr {
            msg: format!("Value out of range {} ", what),
            line: self.line,
            col: self.col,
        }
    }

    pub fn err_type(&self, what: &str) -> DeJsonErr {
        DeJsonErr {
            msg: format!("Token wrong type {} ", what),
            line: self.line,
            col: self.col,
        }
    }

    pub fn err_parse(&self, what: &str) -> DeJsonErr {
        DeJsonErr {
            msg: format!("Cannot parse {} ", what),
            line: self.line,
            col: self.col,
        }
    }

    pub fn eat_comma_block(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match self.tok {
            DeJsonTok::Comma => {
                self.next_tok(i)?;
                Ok(())
            }
            DeJsonTok::BlockClose => Ok(()),
            _ => Err(self.err_token(", or ]")),
        }
    }

    pub fn eat_comma_curly(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match self.tok {
            DeJsonTok::Comma => {
                self.next_tok(i)?;
                Ok(())
            }
            DeJsonTok::CurlyClose => Ok(()),
            _ => Err(self.err_token(", or }")),
        }
    }

    pub fn colon(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match self.tok {
            DeJsonTok::Colon => {
                self.next_tok(i)?;
                Ok(())
            }
            _ => Err(self.err_token(":")),
        }
    }

    pub fn string(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match &mut self.tok {
            DeJsonTok::Str => {
                self.next_tok(i)?;
                Ok(())
            }
            _ => Err(self.err_token("String")),
        }
    }

    pub fn next_colon(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        self.next_tok(i)?;
        self.colon(i)?;
        Ok(())
    }

    pub fn next_str(&mut self) -> Option<()> {
        if let DeJsonTok::Str = &mut self.tok {
            //let mut s = String::new();
            //std::mem::swap(&mut s, name);
            Some(())
        } else {
            None
        }
    }

    /// Every array or object open passes through here, so the depth limit
    /// covers derives, `JsonValue` and `skip_value` alike.
    fn descend(&mut self) -> Result<(), DeJsonErr> {
        if self.depth >= MAX_JSON_DEPTH {
            return Err(self.err_msg(&format!("Nesting deeper than {MAX_JSON_DEPTH} levels")));
        }
        self.depth += 1;
        Ok(())
    }

    pub fn block_open(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        if self.tok == DeJsonTok::BlockOpen {
            self.descend()?;
            self.next_tok(i)?;
            return Ok(());
        }
        Err(self.err_token("["))
    }

    pub fn block_close(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        if self.tok == DeJsonTok::BlockClose {
            self.depth = self.depth.saturating_sub(1);
            self.next_tok(i)?;
            return Ok(());
        }
        Err(self.err_token("]"))
    }

    pub fn curly_open(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        if self.tok == DeJsonTok::CurlyOpen {
            self.descend()?;
            self.next_tok(i)?;
            return Ok(());
        }
        Err(self.err_token("{"))
    }

    pub fn curly_close(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        if self.tok == DeJsonTok::CurlyClose {
            self.depth = self.depth.saturating_sub(1);
            self.next_tok(i)?;
            return Ok(());
        }
        Err(self.err_token("}"))
    }

    pub fn u64_range(&mut self, max: u64) -> Result<u64, DeJsonErr> {
        if let DeJsonTok::U64(value) = self.tok {
            if value > max {
                return Err(self.err_range(&format!("{}>{}", value, max)));
            }
            return Ok(value);
        }
        if let DeJsonTok::U128(value) = self.tok {
            if value > max as u128 {
                return Err(self.err_range(&format!("{}>{}", value, max)));
            }
            return Ok(value as u64);
        }
        Err(self.err_token("unsigned integer"))
    }

    pub fn u128_range(&mut self, max: u128) -> Result<u128, DeJsonErr> {
        if let DeJsonTok::U128(value) = self.tok {
            if value > max {
                return Err(self.err_range(&format!("{}>{}", value, max)));
            }
            return Ok(value);
        }
        if let DeJsonTok::U64(value) = self.tok {
            return Ok(value as u128);
        }
        Err(self.err_token("unsigned integer"))
    }

    pub fn i64_range(&mut self, min: i64, max: i64) -> Result<i64, DeJsonErr> {
        if let DeJsonTok::I64(value) = self.tok {
            if value < min {
                return Err(self.err_range(&format!("{}<{}", value, min)));
            }
            return Ok(value);
        }
        if let DeJsonTok::I128(value) = self.tok {
            if value < min as i128 {
                return Err(self.err_range(&format!("{}<{}", value, min)));
            }
            if value > max as i128 {
                return Err(self.err_range(&format!("{}>{}", value, max)));
            }
            return Ok(value as i64);
        }
        if let DeJsonTok::U64(value) = self.tok {
            if value as i64 > max {
                return Err(self.err_range(&format!("{}>{}", value, max)));
            }
            return Ok(value as i64);
        }
        if let DeJsonTok::U128(value) = self.tok {
            if value > max as u128 {
                return Err(self.err_range(&format!("{}>{}", value, max)));
            }
            return Ok(value as i64);
        }
        Err(self.err_token("signed integer"))
    }

    pub fn i128_range(&mut self, min: i128, max: i128) -> Result<i128, DeJsonErr> {
        if let DeJsonTok::I128(value) = self.tok {
            if value < min {
                return Err(self.err_range(&format!("{}<{}", value, min)));
            }
            if value > max {
                return Err(self.err_range(&format!("{}>{}", value, max)));
            }
            return Ok(value);
        }
        if let DeJsonTok::I64(value) = self.tok {
            return Ok(value as i128);
        }
        if let DeJsonTok::U128(value) = self.tok {
            if value > max as u128 {
                return Err(self.err_range(&format!("{}>{}", value, max)));
            }
            return Ok(value as i128);
        }
        if let DeJsonTok::U64(value) = self.tok {
            return Ok(value as i128);
        }
        Err(self.err_token("signed integer"))
    }

    pub fn as_f64(&mut self) -> Result<f64, DeJsonErr> {
        if let DeJsonTok::I128(value) = self.tok {
            return Ok(value as f64);
        }
        if let DeJsonTok::U128(value) = self.tok {
            return Ok(value as f64);
        }
        if let DeJsonTok::I64(value) = self.tok {
            return Ok(value as f64);
        }
        if let DeJsonTok::U64(value) = self.tok {
            return Ok(value as f64);
        }
        if let DeJsonTok::F64(value) = self.tok {
            return Ok(value);
        }
        Err(self.err_token("floating point"))
    }

    pub fn as_bool(&mut self) -> Result<bool, DeJsonErr> {
        if let DeJsonTok::Bool(value) = self.tok {
            return Ok(value);
        }
        Err(self.err_token("boolean"))
    }

    pub fn as_string(&mut self) -> Result<String, DeJsonErr> {
        if let DeJsonTok::Str = &mut self.tok {
            let mut val = String::new();
            std::mem::swap(&mut val, &mut self.strbuf);
            return Ok(val);
        }
        Err(self.err_token("string"))
    }

    pub fn as_ident(&mut self) -> Result<String, DeJsonErr> {
        if let DeJsonTok::BareIdent = &mut self.tok {
            let mut val = String::new();
            std::mem::swap(&mut val, &mut self.identbuf);
            return Ok(val);
        }
        Err(self.err_token("ident"))
    }

    pub fn skip_value(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        match self.tok {
            DeJsonTok::Str
            | DeJsonTok::U64(_)
            | DeJsonTok::U128(_)
            | DeJsonTok::I64(_)
            | DeJsonTok::I128(_)
            | DeJsonTok::F64(_)
            | DeJsonTok::Bool(_)
            | DeJsonTok::Null => {
                self.next_tok(i)?;
                Ok(())
            }
            DeJsonTok::CurlyOpen => {
                self.curly_open(i)?;
                while self.tok != DeJsonTok::CurlyClose {
                    self.string(i)?;
                    self.colon(i)?;
                    self.skip_value(i)?;
                    self.eat_comma_curly(i)?;
                }
                self.curly_close(i)?;
                Ok(())
            }
            DeJsonTok::BlockOpen => {
                self.block_open(i)?;
                while self.tok != DeJsonTok::BlockClose {
                    self.skip_value(i)?;
                    self.eat_comma_block(i)?;
                }
                self.block_close(i)?;
                Ok(())
            }
            _ => Err(self.err_token("value to skip")),
        }
    }

    pub fn next_tok(&mut self, i: &mut Chars) -> Result<(), DeJsonErr> {
        while self.cur == '\n' || self.cur == '\r' || self.cur == '\t' || self.cur == ' ' {
            self.next(i);
        }
        if self.cur == '\0' {
            self.tok = DeJsonTok::Eof;
            return Ok(());
        }
        match self.cur {
            ':' => {
                self.next(i);
                self.tok = DeJsonTok::Colon;
                Ok(())
            }
            ',' => {
                self.next(i);
                self.tok = DeJsonTok::Comma;
                Ok(())
            }
            '[' => {
                self.next(i);
                self.tok = DeJsonTok::BlockOpen;
                Ok(())
            }
            ']' => {
                self.next(i);
                self.tok = DeJsonTok::BlockClose;
                Ok(())
            }
            '{' => {
                self.next(i);
                self.tok = DeJsonTok::CurlyOpen;
                Ok(())
            }
            '}' => {
                self.next(i);
                self.tok = DeJsonTok::CurlyClose;
                Ok(())
            }
            '-' | '0'..='9' => {
                self.numbuf.clear();
                let is_neg = if self.cur == '-' {
                    self.numbuf.push(self.cur);
                    self.next(i);
                    true
                } else {
                    false
                };
                let mut is_float = false;
                while self.cur >= '0' && self.cur <= '9' {
                    self.numbuf.push(self.cur);
                    self.next(i);
                }
                if self.strict {
                    let digits = &self.numbuf[usize::from(is_neg)..];
                    if digits.len() > 1 && digits.starts_with('0') {
                        return Err(self.err_parse("number with a leading zero"));
                    }
                }
                if self.cur == '.' {
                    is_float = true;
                    self.numbuf.push(self.cur);
                    self.next(i);
                    while self.cur >= '0' && self.cur <= '9' {
                        self.numbuf.push(self.cur);
                        self.next(i);
                    }
                }
                if self.cur == 'e' || self.cur == 'E' {
                    is_float = true;
                    self.numbuf.push(self.cur);
                    self.next(i);
                    if self.cur == '+' || self.cur == '-' {
                        self.numbuf.push(self.cur);
                        self.next(i);
                    }
                    if !(self.cur >= '0' && self.cur <= '9') {
                        return Err(self.err_parse("number"));
                    }
                    while self.cur >= '0' && self.cur <= '9' {
                        self.numbuf.push(self.cur);
                        self.next(i);
                    }
                }
                if is_float {
                    if let Ok(num) = self.numbuf.parse() {
                        self.tok = DeJsonTok::F64(num);
                        Ok(())
                    } else {
                        Err(self.err_parse("number"))
                    }
                } else {
                    if is_neg {
                        if let Ok(num) = self.numbuf.parse() {
                            self.tok = DeJsonTok::I64(num);
                            return Ok(());
                        }
                        if let Ok(num) = self.numbuf.parse() {
                            self.tok = DeJsonTok::I128(num);
                            return Ok(());
                        }
                        return Err(self.err_parse("number"));
                    }
                    if let Ok(num) = self.numbuf.parse() {
                        self.tok = DeJsonTok::U64(num);
                        return Ok(());
                    }
                    if let Ok(num) = self.numbuf.parse() {
                        self.tok = DeJsonTok::U128(num);
                        return Ok(());
                    }
                    Err(self.err_parse("number"))
                }
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                self.identbuf.clear();
                while self.cur >= 'a' && self.cur <= 'z'
                    || self.cur >= 'A' && self.cur <= 'Z'
                    || self.cur == '_'
                {
                    self.identbuf.push(self.cur);
                    self.next(i);
                }
                if self.identbuf == "true" {
                    self.tok = DeJsonTok::Bool(true);
                    return Ok(());
                }
                if self.identbuf == "false" {
                    self.tok = DeJsonTok::Bool(false);
                    return Ok(());
                }
                if self.identbuf == "null" {
                    self.tok = DeJsonTok::Null;
                    return Ok(());
                }
                self.tok = DeJsonTok::BareIdent;
                Err(self.err_token(&format!(
                    "Got ##{}## needed true, false, null",
                    self.identbuf
                )))
            }
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
                            'b' => self.strbuf.push('\u{8}'),
                            'f' => self.strbuf.push('\u{c}'),
                            '0' => self.strbuf.push('\0'),
                            '\0' => {
                                return Err(self.err_parse("string"));
                            }
                            'u' => {
                                // 4 hex digits; a UTF-16 high surrogate is
                                // joined with the `\uXXXX` low surrogate
                                // that must follow it.
                                let mut a = self.hex4(i);
                                if (0xD800..0xDC00).contains(&a) {
                                    // `self.cur` is the last hex digit; look
                                    // past it for the pair.
                                    let mut probe = i.clone();
                                    if probe.next() == Some('\\') && probe.next() == Some('u') {
                                        self.next(i);
                                        self.next(i);
                                        let low = self.hex4(i);
                                        if (0xDC00..0xE000).contains(&low) {
                                            a = 0x10000 + ((a - 0xD800) << 10) + (low - 0xDC00);
                                        }
                                    }
                                }
                                self.strbuf.push(std::char::from_u32(a).unwrap_or('\u{FFFD}'));
                            }
                            _ => self.strbuf.push(self.cur),
                        }
                        self.next(i);
                    } else {
                        if self.cur == '\0' {
                            return Err(self.err_parse("string"));
                        } else {
                            self.strbuf.push(self.cur);
                        }
                        self.next(i);
                    }
                }
                self.next(i);
                self.tok = DeJsonTok::Str;
                Ok(())
            }
            _ => Err(self.err_token("tokenizer")),
        }
    }
}

macro_rules! impl_ser_de_json_unsigned {
    ( $ ty: ident, $ max: expr) => {
        impl SerJson for $ty {
            fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
                s.out.push_str(&self.to_string());
            }
        }

        impl DeJson for $ty {
            fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<$ty, DeJsonErr> {
                let val = s.u128_range($max as u128)?;
                s.next_tok(i)?;
                return Ok(val as $ty);
            }
        }
    };
}

macro_rules! impl_ser_de_json_signed {
    ( $ ty: ident, $ min: expr, $ max: expr) => {
        impl SerJson for $ty {
            fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
                s.out.push_str(&self.to_string());
            }
        }

        impl DeJson for $ty {
            fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<$ty, DeJsonErr> {
                let val = s.i128_range($min as i128, $max as i128)?;
                s.next_tok(i)?;
                return Ok(val as $ty);
            }
        }
    };
}

macro_rules! impl_ser_de_json_float {
    ( $ ty: ident) => {
        impl SerJson for $ty {
            fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
                // JSON has no NaN/inf; they become null, as serde_json does.
                if self.is_finite() {
                    s.out.push_str(&self.to_string());
                } else {
                    s.out.push_str("null");
                }
            }
        }

        impl DeJson for $ty {
            fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<$ty, DeJsonErr> {
                //s.is_prefix(p, i) ?;
                let val = s.as_f64()?;
                s.next_tok(i)?;
                return Ok(val as $ty);
            }
        }
    };
}

impl_ser_de_json_unsigned!(usize, std::u64::MAX);
impl_ser_de_json_unsigned!(u64, std::u64::MAX);
impl_ser_de_json_unsigned!(u128, std::u128::MAX);
impl_ser_de_json_unsigned!(u32, std::u32::MAX);
impl_ser_de_json_unsigned!(u16, std::u16::MAX);
impl_ser_de_json_unsigned!(u8, std::u8::MAX);
impl_ser_de_json_signed!(i64, std::i64::MIN, std::i64::MAX);
impl_ser_de_json_signed!(i128, std::i128::MIN, std::i128::MAX);
impl_ser_de_json_signed!(i32, std::i64::MIN, std::i64::MAX);
impl_ser_de_json_signed!(i16, std::i64::MIN, std::i64::MAX);
impl_ser_de_json_signed!(i8, std::i64::MIN, std::i8::MAX);
impl_ser_de_json_float!(f64);
impl_ser_de_json_float!(f32);

impl SerJson for LiveId {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        self.0.ser_json(d, s);
    }
}

impl DeJson for LiveId {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self, DeJsonErr> {
        Ok(LiveId(u64::de_json(s, i)?))
    }
}

impl<T> SerJson for Option<T>
where
    T: SerJson,
{
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        if let Some(v) = self {
            v.ser_json(d, s);
        } else {
            s.out.push_str("null");
        }
    }
}

impl<T> DeJson for Option<T>
where
    T: DeJson,
{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self, DeJsonErr> {
        if let DeJsonTok::Null = s.tok {
            s.next_tok(i)?;
            return Ok(None);
        }
        Ok(Some(DeJson::de_json(s, i)?))
    }
}

impl SerJson for bool {
    fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
        if *self {
            s.out.push_str("true")
        } else {
            s.out.push_str("false")
        }
    }
}

impl DeJson for bool {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<bool, DeJsonErr> {
        let val = s.as_bool()?;
        s.next_tok(i)?;
        Ok(val)
    }
}

impl SerJson for str {
    fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
        s.out.push('"');
        for c in self.chars() {
            match c {
                '\n' => {
                    s.out.push('\\');
                    s.out.push('n');
                }
                '\r' => {
                    s.out.push('\\');
                    s.out.push('r');
                }
                '\t' => {
                    s.out.push('\\');
                    s.out.push('t');
                }
                '\\' => {
                    s.out.push('\\');
                    s.out.push('\\');
                }
                '"' => {
                    s.out.push('\\');
                    s.out.push('"');
                }
                // The remaining control characters (NUL included) have no
                // short escape in JSON.
                c if (c as u32) < 0x20 => {
                    s.out.push_str(&format!("\\u{:04x}", c as u32));
                }
                _ => s.out.push(c),
            }
        }
        s.out.push('"');
    }
}

impl SerJson for String {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        self.as_str().ser_json(d, s);
    }
}

impl<T> SerJson for &T
where
    T: SerJson + ?Sized,
{
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        (**self).ser_json(d, s);
    }
}

impl DeJson for String {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<String, DeJsonErr> {
        let val = s.as_string()?;
        s.next_tok(i)?;
        Ok(val)
    }
}

impl<T> SerJson for Vec<T>
where
    T: SerJson,
{
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('[');
        if !self.is_empty() {
            let last = self.len() - 1;
            for (index, item) in self.iter().enumerate() {
                s.indent(d + 1);
                item.ser_json(d + 1, s);
                if index != last {
                    s.out.push(',');
                }
            }
            s.indent(d);
        }
        s.out.push(']');
    }
}

impl<T> DeJson for Vec<T>
where
    T: DeJson,
{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Vec<T>, DeJsonErr> {
        let mut out = Vec::new();
        s.block_open(i)?;

        while s.tok != DeJsonTok::BlockClose {
            out.push(DeJson::de_json(s, i)?);
            s.eat_comma_block(i)?;
        }
        s.block_close(i)?;
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    String(String),
    Char(char),
    U64(u64),
    U128(u128),
    I64(i64),
    I128(i128),
    F64(f64),
    Bool(bool),
    BareIdent(String),
    Null,
    Undefined,
    Object(HashMap<String, JsonValue>),
    Array(Vec<JsonValue>),
}

impl JsonValue {
    pub fn object(&self) -> Option<&HashMap<String, JsonValue>> {
        if let JsonValue::Object(obj) = self {
            return Some(obj);
        }
        None
    }
    pub fn string(&self) -> Option<&String> {
        if let JsonValue::String(obj) = self {
            return Some(obj);
        }
        None
    }
    pub fn key(&self, key: &str) -> Option<&JsonValue> {
        if let JsonValue::Object(obj) = self {
            return obj.get(key);
        }
        None
    }

    // --- serde_json-style accessors: `None` whenever the shape differs ---

    /// Member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.key(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut JsonValue> {
        if let JsonValue::Object(obj) = self {
            return obj.get_mut(key);
        }
        None
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(v) => Some(v.as_str()),
            _ => None,
        }
    }

    /// Any integer that fits an i64 (an in-range float is not an integer).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::I64(v) => Some(*v),
            JsonValue::U64(v) => i64::try_from(*v).ok(),
            JsonValue::I128(v) => i64::try_from(*v).ok(),
            JsonValue::U128(v) => i64::try_from(*v).ok(),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            JsonValue::U64(v) => Some(*v),
            JsonValue::I64(v) => u64::try_from(*v).ok(),
            JsonValue::I128(v) => u64::try_from(*v).ok(),
            JsonValue::U128(v) => u64::try_from(*v).ok(),
            _ => None,
        }
    }

    /// Any number, integers widened.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::F64(v) => Some(*v),
            JsonValue::U64(v) => Some(*v as f64),
            JsonValue::I64(v) => Some(*v as f64),
            JsonValue::U128(v) => Some(*v as f64),
            JsonValue::I128(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<JsonValue>> {
        match self {
            JsonValue::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<JsonValue>> {
        match self {
            JsonValue::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&HashMap<String, JsonValue>> {
        self.object()
    }

    pub fn as_object_mut(&mut self) -> Option<&mut HashMap<String, JsonValue>> {
        match self {
            JsonValue::Object(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, JsonValue::Null)
    }

    pub fn is_string(&self) -> bool {
        matches!(self, JsonValue::String(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(
            self,
            JsonValue::U64(_)
                | JsonValue::I64(_)
                | JsonValue::U128(_)
                | JsonValue::I128(_)
                | JsonValue::F64(_)
        )
    }

    pub fn is_array(&self) -> bool {
        matches!(self, JsonValue::Array(_))
    }

    pub fn is_object(&self) -> bool {
        matches!(self, JsonValue::Object(_))
    }
}

impl From<&str> for JsonValue {
    fn from(v: &str) -> Self {
        JsonValue::String(v.to_string())
    }
}

impl From<String> for JsonValue {
    fn from(v: String) -> Self {
        JsonValue::String(v)
    }
}

impl From<bool> for JsonValue {
    fn from(v: bool) -> Self {
        JsonValue::Bool(v)
    }
}

impl From<f64> for JsonValue {
    fn from(v: f64) -> Self {
        JsonValue::F64(v)
    }
}

impl From<f32> for JsonValue {
    fn from(v: f32) -> Self {
        JsonValue::F64(v as f64)
    }
}

impl From<i64> for JsonValue {
    fn from(v: i64) -> Self {
        JsonValue::I64(v)
    }
}

impl From<i32> for JsonValue {
    fn from(v: i32) -> Self {
        JsonValue::I64(v as i64)
    }
}

impl From<u64> for JsonValue {
    fn from(v: u64) -> Self {
        JsonValue::U64(v)
    }
}

impl From<u32> for JsonValue {
    fn from(v: u32) -> Self {
        JsonValue::U64(v as u64)
    }
}

impl From<usize> for JsonValue {
    fn from(v: usize) -> Self {
        JsonValue::U64(v as u64)
    }
}

impl From<Vec<JsonValue>> for JsonValue {
    fn from(v: Vec<JsonValue>) -> Self {
        JsonValue::Array(v)
    }
}

impl From<HashMap<String, JsonValue>> for JsonValue {
    fn from(v: HashMap<String, JsonValue>) -> Self {
        JsonValue::Object(v)
    }
}

impl<T> From<Option<T>> for JsonValue
where
    T: Into<JsonValue>,
{
    fn from(v: Option<T>) -> Self {
        match v {
            Some(v) => v.into(),
            None => JsonValue::Null,
        }
    }
}

/// The compact JSON text (`serialize_json`).
impl std::fmt::Display for JsonValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.serialize_json())
    }
}

impl SerJson for JsonValue {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        match self {
            JsonValue::String(v) => v.ser_json(d, s),
            JsonValue::Char(v) => v.to_string().ser_json(d, s),
            JsonValue::U64(v) => v.ser_json(d, s),
            JsonValue::U128(v) => v.ser_json(d, s),
            JsonValue::I64(v) => v.ser_json(d, s),
            JsonValue::I128(v) => v.ser_json(d, s),
            JsonValue::F64(v) => v.ser_json(d, s),
            JsonValue::Bool(v) => v.ser_json(d, s),
            JsonValue::BareIdent(v) => v.ser_json(d, s),
            JsonValue::Null => s.out.push_str("null"),
            JsonValue::Undefined => s.out.push_str("undefined"),
            JsonValue::Object(v) => {
                // Sorted keys: the text of an object is deterministic even
                // though the map is hashed, so it can be diffed and hashed.
                let mut keys: Vec<&String> = v.keys().collect();
                keys.sort();
                s.out.push('{');
                let last = keys.len().saturating_sub(1);
                for (index, k) in keys.iter().enumerate() {
                    s.indent(d + 1);
                    k.ser_json(d + 1, s);
                    s.out.push(':');
                    if s.pretty {
                        s.out.push(' ');
                    }
                    v[*k].ser_json(d + 1, s);
                    if index != last {
                        s.conl();
                    }
                }
                if !keys.is_empty() {
                    s.indent(d);
                }
                s.out.push('}');
            }
            JsonValue::Array(v) => v.ser_json(d, s),
        }
    }
}

impl DeJson for JsonValue {
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<JsonValue, DeJsonErr> {
        // lets check what tokenm we have
        match s.tok {
            DeJsonTok::Str => {
                let v = s.as_string()?;
                s.next_tok(i)?;
                Ok(JsonValue::String(v))
            }
            DeJsonTok::Char(c) => {
                s.next_tok(i)?;
                Ok(JsonValue::Char(c))
            }
            DeJsonTok::U64(v) => {
                s.next_tok(i)?;
                Ok(JsonValue::U64(v))
            }
            DeJsonTok::U128(v) => {
                s.next_tok(i)?;
                Ok(JsonValue::U128(v))
            }
            DeJsonTok::I64(v) => {
                s.next_tok(i)?;
                Ok(JsonValue::I64(v))
            }
            DeJsonTok::I128(v) => {
                s.next_tok(i)?;
                Ok(JsonValue::I128(v))
            }
            DeJsonTok::F64(v) => {
                s.next_tok(i)?;
                Ok(JsonValue::F64(v))
            }
            DeJsonTok::Bool(v) => {
                s.next_tok(i)?;
                Ok(JsonValue::Bool(v))
            }
            DeJsonTok::BareIdent => {
                let v = s.as_ident()?;
                s.next_tok(i)?;
                Ok(JsonValue::BareIdent(v))
            }
            DeJsonTok::Null => {
                s.next_tok(i)?;
                Ok(JsonValue::Null)
            }
            DeJsonTok::Colon => return Err(s.err_msg("Unexpected :")),
            DeJsonTok::CurlyOpen => {
                let mut h = HashMap::new();
                s.curly_open(i)?;
                while s.tok != DeJsonTok::CurlyClose {
                    let k = String::de_json(s, i)?;
                    s.colon(i)?;
                    let v = JsonValue::de_json(s, i)?;
                    s.eat_comma_curly(i)?;
                    h.insert(k, v);
                }
                s.curly_close(i)?;
                Ok(JsonValue::Object(h))
            }
            DeJsonTok::CurlyClose => return Err(s.err_msg("Unexpected }")),
            DeJsonTok::BlockOpen => {
                let mut out = Vec::new();
                s.block_open(i)?;

                while s.tok != DeJsonTok::BlockClose {
                    out.push(JsonValue::de_json(s, i)?);
                    s.eat_comma_block(i)?;
                }
                s.block_close(i)?;
                Ok(JsonValue::Array(out))
            }
            DeJsonTok::BlockClose => return Err(s.err_msg("Unexpected ]")),
            DeJsonTok::Comma => return Err(s.err_msg("Unexpected ,")),
            DeJsonTok::Bof => return Err(s.err_msg("Unexpected Bof")),
            DeJsonTok::Eof => return Err(s.err_msg("Unexpected Eof")),
        }
    }
}

impl<T> SerJson for [T]
where
    T: SerJson,
{
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('[');
        let last = self.len().saturating_sub(1);
        for (index, item) in self.iter().enumerate() {
            item.ser_json(d + 1, s);
            if index != last {
                s.out.push(',');
            }
        }
        s.out.push(']');
    }
}

unsafe fn de_json_array_impl_inner<T>(
    top: *mut T,
    count: usize,
    s: &mut DeJsonState,
    i: &mut Chars,
) -> Result<(), DeJsonErr>
where
    T: DeJson,
{
    s.block_open(i)?;
    for c in 0..count {
        top.add(c).write(DeJson::de_json(s, i)?);
        s.eat_comma_block(i)?;
    }
    s.block_close(i)?;
    Ok(())
}

impl<T, const N: usize> DeJson for [T; N]
where
    T: DeJson,
{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self, DeJsonErr> {
        unsafe {
            let mut to = std::mem::MaybeUninit::<[T; N]>::uninit();
            let top: *mut T = &mut to as *mut _ as *mut T;
            de_json_array_impl_inner(top, N, s, i)?;
            Ok(to.assume_init())
        }
    }
}

fn de_json_comma_block<T>(s: &mut DeJsonState, i: &mut Chars) -> Result<T, DeJsonErr>
where
    T: DeJson,
{
    let t = DeJson::de_json(s, i);
    s.eat_comma_block(i)?;
    t
}

impl<A, B> SerJson for (A, B)
where
    A: SerJson,
    B: SerJson,
{
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('[');
        self.0.ser_json(d, s);
        s.out.push(',');
        self.1.ser_json(d, s);
        s.out.push(']');
    }
}

impl<A, B> DeJson for (A, B)
where
    A: DeJson,
    B: DeJson,
{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<(A, B), DeJsonErr> {
        s.block_open(i)?;
        let r = (de_json_comma_block(s, i)?, de_json_comma_block(s, i)?);
        s.block_close(i)?;
        Ok(r)
    }
}

impl<A, B, C> SerJson for (A, B, C)
where
    A: SerJson,
    B: SerJson,
    C: SerJson,
{
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

impl<A, B, C> DeJson for (A, B, C)
where
    A: DeJson,
    B: DeJson,
    C: DeJson,
{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<(A, B, C), DeJsonErr> {
        s.block_open(i)?;
        let r = (
            de_json_comma_block(s, i)?,
            de_json_comma_block(s, i)?,
            de_json_comma_block(s, i)?,
        );
        s.block_close(i)?;
        Ok(r)
    }
}

impl<A, B, C, D> SerJson for (A, B, C, D)
where
    A: SerJson,
    B: SerJson,
    C: SerJson,
    D: SerJson,
{
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

impl<A, B, C, D> DeJson for (A, B, C, D)
where
    A: DeJson,
    B: DeJson,
    C: DeJson,
    D: DeJson,
{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<(A, B, C, D), DeJsonErr> {
        s.block_open(i)?;
        let r = (
            de_json_comma_block(s, i)?,
            de_json_comma_block(s, i)?,
            de_json_comma_block(s, i)?,
            de_json_comma_block(s, i)?,
        );
        s.block_close(i)?;
        Ok(r)
    }
}

impl<K, V> SerJson for HashMap<K, V>
where
    K: SerJson,
    V: SerJson,
{
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.out.push('{');
        let last = self.len().saturating_sub(1);
        for (index, (k, v)) in self.iter().enumerate() {
            s.indent(d + 1);
            k.ser_json(d + 1, s);
            s.out.push(':');
            if s.pretty {
                s.out.push(' ');
            }
            v.ser_json(d + 1, s);
            if index != last {
                s.conl();
            }
        }
        if !self.is_empty() {
            s.indent(d);
        }
        s.out.push('}');
    }
}

impl<K, V> DeJson for HashMap<K, V>
where
    K: DeJson + Eq + Hash,
    V: DeJson,
{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Self, DeJsonErr> {
        let mut h = HashMap::new();
        s.curly_open(i)?;
        while s.tok != DeJsonTok::CurlyClose {
            let k = DeJson::de_json(s, i)?;
            s.colon(i)?;
            let v = DeJson::de_json(s, i)?;
            s.eat_comma_curly(i)?;
            h.insert(k, v);
        }
        s.curly_close(i)?;
        Ok(h)
    }
}

impl<T> SerJson for Box<T>
where
    T: SerJson,
{
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        (**self).ser_json(d, s)
    }
}

impl<T> DeJson for Box<T>
where
    T: DeJson,
{
    fn de_json(s: &mut DeJsonState, i: &mut Chars) -> Result<Box<T>, DeJsonErr> {
        Ok(Box::new(DeJson::de_json(s, i)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_micro_serde_derive::{DeJson, SerJson};
    use std::collections::HashMap;

    #[test]
    fn serialize_empty_hashmap_json() {
        let map: HashMap<String, String> = HashMap::new();
        assert_eq!(map.serialize_json(), "{}");
    }

    #[test]
    fn deserialize_f64_scientific_notation() {
        let v: f64 = DeJson::deserialize_json("1.25e-2").unwrap();
        assert!((v - 0.0125).abs() < 1e-12);

        let v: f64 = DeJson::deserialize_json("-3E+1").unwrap();
        assert!((v + 30.0).abs() < 1e-12);
    }

    #[test]
    fn deserialize_u128_integer() {
        let value: u128 = DeJson::deserialize_json("1000000000000000019884624838656").unwrap();
        assert_eq!(value, 1000000000000000019884624838656u128);
    }

    #[test]
    fn deserialize_string_preserves_dollar_signs() {
        let value: String = DeJson::deserialize_json("\"▁$($\"").unwrap();
        assert_eq!(value, "▁$($");
    }

    #[test]
    fn string_escapes_round_trip() {
        let text = "tab\t nl\n cr\r bs\u{8} ff\u{c} nul\0 quote\" slash\\ bell\u{7}";
        let json = text.serialize_json();
        assert_eq!(
            json,
            "\"tab\\t nl\\n cr\\r bs\\u0008 ff\\u000c nul\\u0000 quote\\\" slash\\\\ bell\\u0007\""
        );
        let back: String = DeJson::deserialize_json(&json).unwrap();
        assert_eq!(back, text);
        // The short escapes JSON defines, and the legacy `\0`.
        let back: String = DeJson::deserialize_json(r#""\b\f\/\0""#).unwrap();
        assert_eq!(back, "\u{8}\u{c}/\0");
    }

    #[test]
    fn unicode_escapes_join_surrogate_pairs() {
        let back: String = DeJson::deserialize_json(r#""a😀bé""#).unwrap();
        assert_eq!(back, "a😀bé");
        // A lone surrogate becomes U+FFFD instead of corrupting the string.
        let back: String = DeJson::deserialize_json(r#""x\ud83dy""#).unwrap();
        assert_eq!(back, "x\u{FFFD}y");
    }

    #[test]
    fn option_none_and_non_finite_floats_serialize_as_null() {
        let values: Vec<Option<u32>> = vec![Some(1), None];
        assert_eq!(values.serialize_json(), "[1,null]");
        assert_eq!(f64::NAN.serialize_json(), "null");
        assert_eq!(f64::INFINITY.serialize_json(), "null");
        assert_eq!(2.5f64.serialize_json(), "2.5");
    }

    #[test]
    fn str_and_references_serialize() {
        assert_eq!("hi".serialize_json(), "\"hi\"");
        let map: HashMap<String, &str> = [("k".to_string(), "v")].into_iter().collect();
        assert_eq!(map.serialize_json(), "{\"k\":\"v\"}");
    }

    #[derive(SerJson, DeJson, PartialEq, Debug)]
    struct Record {
        event: String,
        t: f64,
        speed: Option<f64>,
        tags: Vec<String>,
    }

    fn nested(open: &str, close: &str, depth: usize) -> String {
        let mut text = String::new();
        for _ in 0..depth {
            text.push_str(open);
        }
        text.push_str("1");
        for _ in 0..depth {
            text.push_str(close);
        }
        text
    }

    #[test]
    fn nesting_is_capped_at_the_depth_limit() {
        let deep = MAX_JSON_DEPTH as usize;
        assert!(JsonValue::deserialize_json(&nested("[", "]", deep)).is_ok());
        assert!(JsonValue::deserialize_json(&nested("[", "]", deep + 1))
            .unwrap_err()
            .msg
            .contains("Nesting"));
        assert!(JsonValue::deserialize_json(&nested("{\"a\":", "}", deep)).is_ok());
        assert!(JsonValue::deserialize_json(&nested("{\"a\":", "}", deep + 1)).is_err());
        // A derive skipping an unknown field in lenient mode descends too.
        let record = |extra: &str| {
            format!(r#"{{"event":"x","t":1,"tags":[],"extra":{extra}}}"#)
        };
        assert!(Record::deserialize_json_lenient(&record(&nested("[", "]", deep - 1))).is_ok());
        assert!(Record::deserialize_json_lenient(&record(&nested("[", "]", deep))).is_err());
        // Closing brackets give the depth back, so siblings do not add up.
        let siblings = format!("[{},{}]", nested("[", "]", deep - 1), nested("[", "]", deep - 1));
        assert!(JsonValue::deserialize_json(&siblings).is_ok());
    }

    #[test]
    fn strict_parse_rejects_trailing_content_and_leading_zeros() {
        assert!(JsonValue::deserialize_json_strict(r#"{"a":1}{"#).is_err());
        assert!(JsonValue::deserialize_json_strict("[1, 2] {note}").is_err());
        assert!(JsonValue::deserialize_json_strict("01").is_err());
        assert!(JsonValue::deserialize_json_strict("-01").is_err());
        assert_eq!(
            JsonValue::deserialize_json_strict(" {\"a\": 0.5} \n").unwrap().get("a").and_then(|v| v.as_f64()),
            Some(0.5)
        );
        assert_eq!(JsonValue::deserialize_json_strict("0").unwrap().as_u64(), Some(0));
        assert_eq!(JsonValue::deserialize_json_strict("-0").unwrap().as_i64(), Some(0));
        // The default parse keeps ignoring what follows the value.
        assert!(JsonValue::deserialize_json(r#"{"a":1}{"#).is_ok());
    }

    #[test]
    fn pretty_output_is_indented_and_parses_back() {
        let record = Record {
            event: "fix".into(),
            t: 12.5,
            speed: None,
            tags: vec!["a".into(), "b".into()],
        };
        assert_eq!(record.serialize_json(), r#"{"event":"fix","t":12.5,"tags":["a","b"]}"#);
        let pretty = record.serialize_json_pretty();
        assert_eq!(
            pretty,
            "{\n    \"event\": \"fix\",\n    \"t\": 12.5,\n    \"tags\": [\n        \"a\",\n        \"b\"\n    ]\n}"
        );
        assert_eq!(Record::deserialize_json(&pretty).unwrap(), record);
        let empty: Vec<u8> = Vec::new();
        assert_eq!(empty.serialize_json_pretty(), "[]");
        let empty: HashMap<String, u8> = HashMap::new();
        assert_eq!(empty.serialize_json_pretty(), "{}");
    }

    #[test]
    fn json_value_accessors_and_sorted_output() {
        let value = JsonValue::deserialize_json(
            r#"{"z":1,"a":"x","n":-2,"f":1.5,"b":true,"nil":null,"arr":[1,2.5,"s"],"big":18446744073709551615}"#,
        )
        .unwrap();
        assert_eq!(value.get("a").and_then(|v| v.as_str()), Some("x"));
        assert_eq!(value.get("z").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(value.get("z").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(value.get("z").and_then(|v| v.as_f64()), Some(1.0));
        assert_eq!(value.get("n").and_then(|v| v.as_i64()), Some(-2));
        assert_eq!(value.get("n").and_then(|v| v.as_u64()), None);
        assert_eq!(value.get("f").and_then(|v| v.as_f64()), Some(1.5));
        assert_eq!(value.get("f").and_then(|v| v.as_i64()), None);
        assert_eq!(value.get("b").and_then(|v| v.as_bool()), Some(true));
        assert!(value.get("nil").unwrap().is_null());
        assert_eq!(value.get("big").and_then(|v| v.as_i64()), None);
        assert_eq!(value.get("big").and_then(|v| v.as_u64()), Some(u64::MAX));
        assert_eq!(value.get("arr").and_then(|v| v.as_array()).map(|a| a.len()), Some(3));
        assert_eq!(value.get("missing"), None);
        assert_eq!(JsonValue::Null.get("k"), None);
        assert_eq!(
            value.to_string(),
            r#"{"a":"x","arr":[1,2.5,"s"],"b":true,"big":18446744073709551615,"f":1.5,"n":-2,"nil":null,"z":1}"#
        );
        let mut value = value;
        value.as_object_mut().unwrap().remove("big");
        value.as_object_mut().unwrap().insert("o".into(), JsonValue::from(Some(3u32)));
        assert_eq!(
            format!("{value}"),
            r#"{"a":"x","arr":[1,2.5,"s"],"b":true,"f":1.5,"n":-2,"nil":null,"o":3,"z":1}"#
        );
        assert_eq!(JsonValue::from(None::<u32>), JsonValue::Null);
        assert_eq!(JsonValue::from("s"), JsonValue::String("s".into()));
    }
}
