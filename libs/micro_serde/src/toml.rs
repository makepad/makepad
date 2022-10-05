use std::collections::{HashMap};
use std::str::Chars;

#[derive(Default)]
pub struct TomlParser {
    pub cur: char,
    pub line: usize,
    pub col: usize
}

#[derive(PartialEq, Debug)]
pub enum TomlTok {
    Ident(String),
    Str(String),
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    Nan(bool),
    Inf(bool),
    Date(String),
    Equals,
    BlockOpen,
    BlockClose,
    Comma,
    Bof,
    Eof
}

pub enum Toml{
    Str(String),
    Bool(bool),
    Num(f64),
    Date(String),
    Array(Vec<Toml>),
}

pub struct TomlErr{
    pub msg:String,
    pub line:usize,
    pub col:usize
}

impl std::fmt::Debug for TomlErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Toml error: {}, line:{} col:{}", self.msg, self.line+1, self.col+1)
    }
}

impl TomlParser {
    pub fn to_val(&mut self, tok:TomlTok, i: &mut Chars)->Result<Toml, TomlErr>{
        match tok{
            TomlTok::BlockOpen=>{
                let mut vals = Vec::new();
                loop{
                    let tok = self.next_tok(i)?;
                    if tok == TomlTok::BlockClose || tok == TomlTok::Eof{
                        break
                    }
                    if tok != TomlTok::Comma{
                        vals.push(self.to_val(tok, i)?);
                    }
                }
                Ok(Toml::Array(vals))
            },
            TomlTok::Str(v)=>Ok(Toml::Str(v)),
            TomlTok::U64(v)=>Ok(Toml::Num(v as f64)),
            TomlTok::I64(v)=>Ok(Toml::Num(v as f64)),
            TomlTok::F64(v)=>Ok(Toml::Num(v as f64)),
            TomlTok::Bool(v)=>Ok(Toml::Bool(v)),
            TomlTok::Nan(v)=>Ok(Toml::Num(if v{-std::f64::NAN}else{std::f64::NAN})),
            TomlTok::Inf(v)=>Ok(Toml::Num(if v{-std::f64::INFINITY}else{std::f64::INFINITY})),
            TomlTok::Date(v)=>Ok(Toml::Date(v)),
            _=>Err(self.err_token(tok))
        }
    }
    
    pub fn parse_key_value(&mut self, local_scope:&String, key:String, i: &mut Chars, out:&mut HashMap<String, Toml>)->Result<(), TomlErr>{
        let tok = self.next_tok(i)?;
        if tok != TomlTok::Equals{
            return Err(self.err_token(tok));
        }
        let tok = self.next_tok(i)?;
        let val = self.to_val(tok, i)?;
        let key = if local_scope.len()>0{
            format!("{}.{}", local_scope, key)
        }
        else{
            key
        };
        out.insert(key, val);
        Ok(())
    }
    
    pub fn parse(data:&str)->Result<HashMap<String, Toml>, TomlErr>{
        let i = &mut data.chars();
        let mut t = TomlParser::default();
        t.next(i);
        let mut out = HashMap::new();
        let mut local_scope = String::new();
        loop{
            let tok = t.next_tok(i)?;
            match tok{
                TomlTok::Eof=>{ // at eof. 
                    return Ok(out);
                },
                TomlTok::BlockOpen=>{ // its a scope
                    // we should expect an ident or a string
                    let tok = t.next_tok(i)?;
                    match tok{
                        TomlTok::Str(key)=>{ // a key
                            local_scope = key;
                        },
                        TomlTok::Ident(key)=>{ // also a key
                            local_scope = key;
                        },
                        _=>return Err(t.err_token(tok))
                    }
                    let tok = t.next_tok(i)?;
                    if tok != TomlTok::BlockClose{
                        return Err(t.err_token(tok))
                    }
                },
                TomlTok::Str(key)=>{ // a key
                    t.parse_key_value(&local_scope, key, i, &mut out)?;
                },
                TomlTok::Ident(key)=>{ // also a key
                    t.parse_key_value(&local_scope, key, i, &mut out)?;
                },
                _=>return Err(t.err_token(tok))
            }
        }
    }
    
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
    
    pub fn err_token(&self, tok:TomlTok) -> TomlErr {
        TomlErr{msg:format!("Unexpected token {:?} ", tok), line:self.line, col:self.col}
    }
    
    pub fn err_parse(&self, what:&str) -> TomlErr {
        TomlErr{msg:format!("Cannot parse toml {} ", what), line:self.line, col:self.col}
    }
    
    pub fn next_tok(&mut self, i: &mut Chars) -> Result<TomlTok, TomlErr> {
        while self.cur == '\n' || self.cur == '\r' || self.cur == '\t' || self.cur == ' ' {
            self.next(i);
        }
        loop{
            if self.cur == '\0' {
                return Ok(TomlTok::Eof)
            }
            match self.cur {
                ',' => {
                    self.next(i);
                    return Ok(TomlTok::Comma)
                }
                '[' => {
                    self.next(i);
                    return Ok(TomlTok::BlockOpen)
                }
                ']' => {
                    self.next(i);
                    return Ok(TomlTok::BlockClose)
                }
                '=' => {
                    self.next(i);
                    return Ok(TomlTok::Equals)
                }
                '+' | '-' | '0'..='9' => {
                    let mut num = String::new();
                    let is_neg = if self.cur == '-' {
                        num.push(self.cur);
                        self.next(i);
                        true
                    }
                    else {
                        if self.cur == '+' {
                            self.next(i);
                        }
                        false
                    };
                    if self.cur == 'n' {
                        self.next(i);
                        if self.cur == 'a' {
                            self.next(i);
                            if self.cur == 'n' {
                                self.next(i);
                                return Ok(TomlTok::Nan(is_neg))
                            }
                            else {
                                return Err(self.err_parse("nan"))
                            }
                        }
                        else {
                            return Err(self.err_parse("nan"))
                        }
                    }
                     if self.cur == 'i' {
                        self.next(i);
                        if self.cur == 'n' {
                            self.next(i);
                            if self.cur == 'f' {
                                self.next(i);
                                return Ok(TomlTok::Inf(is_neg))
                            }
                            else {
                                return Err(self.err_parse("inf"))
                            }
                        }
                        else {
                            return Err(self.err_parse("nan"))
                        }
                    }
                    while self.cur >= '0' && self.cur <= '9' || self.cur == '_' {
                        if self.cur != '_' {
                            num.push(self.cur);
                        }
                        self.next(i);
                    }
                    if self.cur == '.' {
                        num.push(self.cur);
                        self.next(i);
                        while self.cur >= '0' && self.cur <= '9' || self.cur == '_' {
                            if self.cur != '_' {
                                num.push(self.cur);
                            }
                            self.next(i);
                        }
                        if let Ok(num) = num.parse() {
                            return Ok(TomlTok::F64(num))
                        }
                        else {
                            return Err(self.err_parse("number"));
                        }
                    }
                    else if self.cur == '-' { // lets assume its a date. whatever. i don't feel like more parsing today
                        num.push(self.cur);
                        self.next(i);
                        while self.cur >= '0' && self.cur <= '9' || self.cur == ':' || self.cur == '-' || self.cur == 'T' {
                            num.push(self.cur);
                            self.next(i);
                        }
                        return Ok(TomlTok::Date(num))
                    }
                    else {
                        if is_neg {
                            if let Ok(num) = num.parse() {
                                 return Ok(TomlTok::I64(num))
                            }
                            else {
                                return Err(self.err_parse("number"));
                            }
                        }
                        if let Ok(num) = num.parse() {
                            return Ok(TomlTok::U64(num))
                        }
                        else {
                            return Err(self.err_parse("number"));
                        }
                    }
                },
                'a'..='z' | 'A'..='Z' | '_' => {
                    let mut ident = String::new();
                    while self.cur >= 'a' && self.cur <= 'z'
                        || self.cur >= 'A' && self.cur <= 'Z'
                        || self.cur == '_' || self.cur == '-' {
                        ident.push(self.cur);
                        self.next(i);
                    }
                    if self.cur == '.' {
                        while self.cur == '.' {
                            self.next(i);
                            while self.cur >= 'a' && self.cur <= 'z'
                                || self.cur >= 'A' && self.cur <= 'Z'
                                || self.cur == '_' || self.cur == '-' {
                                ident.push(self.cur);
                                self.next(i);
                            }
                        }
                        return Ok(TomlTok::Ident(ident))
                    }
                    if ident == "true" {
                        return Ok(TomlTok::Bool(true))
                    }
                    if ident == "false" {
                        return Ok(TomlTok::Bool(false))
                    }
                    if ident == "inf" {
                        return Ok(TomlTok::Inf(false))
                    }
                    if ident == "nan" {
                        return Ok(TomlTok::Nan(false))
                    }
                    return Ok(TomlTok::Ident(ident))
                },
                '#' =>{
                    while self.cur !='\n' && self.cur != '\0'{
                        self.next(i);
                    }
                },
                '"' => {
                    let mut val = String::new();
                    self.next(i);
                    while self.cur != '"' {
                        if self.cur == '\\' {
                            self.next(i);
                        }
                        if self.cur == '\0' {
                            return Err(self.err_parse("string"));
                        }
                        val.push(self.cur);
                        self.next(i);
                    }
                    self.next(i);
                    return Ok(TomlTok::Str(val))
                },
                _ => {
                    return Err(self.err_parse("tokenizer"));
                }
            }
        }
    }
}