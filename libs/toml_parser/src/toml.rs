use std::collections::{HashMap};
use std::str::Chars;

#[derive(Default)]
pub struct TomlParser {
    pub cur: char,
    pub pos: usize,
}

#[derive(PartialEq, Debug, Clone)]
pub struct TomlSpan{
    pub start:usize,
    pub len:usize
}

#[derive(PartialEq, Debug)]
pub struct TomlTokWithSpan{
    pub span: TomlSpan,
    pub tok: TomlTok
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
    ObjectOpen,
    ObjectClose,
    Comma,
    Bof,
    Eof
}


#[derive(PartialEq, Debug, Clone)]
pub enum Toml{
    Str(String, TomlSpan),
    Bool(bool, TomlSpan),
    Num(f64, TomlSpan),
    Date(String, TomlSpan),
    Array(Vec<Toml>),
}

impl Toml{
    pub fn into_str(self)->Option<String>{
        match self{
            Self::Str(v,_)=>Some(v),
            _=>None
        }
    }
}

pub struct TomlErr{
    pub msg:String,
    pub span:TomlSpan,
}

impl std::fmt::Debug for TomlErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Toml error: {}, start:{} len:{}", self.msg, self.span.start, self.span.len)
    }
}

pub fn parse_toml(data:&str)->Result<HashMap<String, Toml>, TomlErr>{
    let i = &mut data.chars();
    let mut t = TomlParser::default();
    t.next(i);
    let mut out = HashMap::new();
    let mut local_scope = String::new();
    loop{
        let tok = t.next_tok(i)?;
        match tok.tok{
            TomlTok::Eof=>{ // at eof. 
                return Ok(out);
            },
            TomlTok::BlockOpen=>{ // its a scope
                // we should expect an ident or a string
                let tok = t.next_tok(i)?;
                let (tok, double_block) = if let TomlTok::BlockOpen = tok.tok{
                    (t.next_tok(i)?, true)
                }
                else{(tok, false)};
                
                match tok.tok{
                    TomlTok::Str(key)=>{ // a key
                        local_scope = key;
                    },
                    TomlTok::Ident(key)=>{ // also a key
                        local_scope = key;
                    },
                    _=>return Err(t.err_token(tok))
                }
                let tok = t.next_tok(i)?;
                
                if tok.tok != TomlTok::BlockClose{
                    return Err(t.err_token(tok))
                }
                if double_block{
                    let tok = t.next_tok(i)?;
                    if tok.tok != TomlTok::BlockClose{
                        return Err(t.err_token(tok))
                    }
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

impl TomlParser {
    pub fn to_val(&mut self, tok:TomlTokWithSpan,  i: &mut Chars)->Result<Toml, TomlErr>{
        match tok.tok{
            TomlTok::BlockOpen=>{
                let mut vals = Vec::new();
                loop{
                    let tok = self.next_tok(i)?;
                    match tok.tok{
                        TomlTok::BlockClose | TomlTok::Eof=>{
                            break;
                        }
                        TomlTok::Comma=>{}
                        _=>{
                            vals.push(self.to_val(tok, i)?);
                        }
                    }
                }
                Ok(Toml::Array(vals))
            },
            TomlTok::Str(v)=>Ok(Toml::Str(v, tok.span)),
            TomlTok::U64(v)=>Ok(Toml::Num(v as f64, tok.span)),
            TomlTok::I64(v)=>Ok(Toml::Num(v as f64, tok.span)),
            TomlTok::F64(v)=>Ok(Toml::Num(v as f64, tok.span)),
            TomlTok::Bool(v)=>Ok(Toml::Bool(v, tok.span)),
            TomlTok::Nan(v)=>Ok(Toml::Num(if v{-std::f64::NAN}else{std::f64::NAN}, tok.span)),
            TomlTok::Inf(v)=>Ok(Toml::Num(if v{-std::f64::INFINITY}else{std::f64::INFINITY}, tok.span)),
            TomlTok::Date(v)=>Ok(Toml::Date(v, tok.span)),
            _=>Err(self.err_token(tok))
        }
    }
    
    pub fn parse_key_value(&mut self, local_scope:&str, key:String, i: &mut Chars, out:&mut HashMap<String, Toml>)->Result<(), TomlErr>{
        let tok = self.next_tok(i)?;
        if tok.tok != TomlTok::Equals{
            return Err(self.err_token(tok));
        }
        let tok = self.next_tok(i)?;
        // if we are an ObjectOpen we do a subscope
        if let TomlTok::ObjectOpen = tok.tok{
            let local_scope = if local_scope.len()>0{
                format!("{}.{}", local_scope, key)
            }
            else{
                key
            };
            loop{
                let tok = self.next_tok(i)?;
                match tok.tok{
                    TomlTok::ObjectClose | TomlTok::Eof=>{
                        break;
                    }
                    TomlTok::Comma=>{}
                    TomlTok::Str(key)=>{ // a key
                        self.parse_key_value(&local_scope, key, i, out)?;
                    },
                    TomlTok::Ident(key)=>{ // also a key
                        self.parse_key_value(&local_scope, key, i, out)?;
                    },
                     _=>return Err(self.err_token(tok))
                }
            }
        }
        else{
            let val = self.to_val(tok, i)?;
            let key = if local_scope.len()>0{
                format!("{}.{}", local_scope, key)
            }
            else{
                key
            };
            out.insert(key, val);
        }
        Ok(())
    }
    
    pub fn next(&mut self, i: &mut Chars) {
        if let Some(c) = i.next() {
            self.cur = c;
            self.pos += 1;
        }
        else {
            self.cur = '\0';
        }
    }
    
    pub fn err_token(&self, tok:TomlTokWithSpan) -> TomlErr {
        TomlErr{msg:format!("Unexpected token {:?} ", tok), span:tok.span}
    }
    
    pub fn err_parse(&self, what:&str) -> TomlErr {
        TomlErr{msg:format!("Cannot parse toml {} ", what), span:TomlSpan{start:self.pos, len:0}}
    }
    
    pub fn next_tok(&mut self, i: &mut Chars) -> Result<TomlTokWithSpan, TomlErr> {
        
        while self.cur == '\n' || self.cur == '\r' || self.cur == '\t' || self.cur == ' ' || self.cur == '#'{
            if self.cur == '#'{
                while self.cur !='\n' && self.cur != '\0'{
                    self.next(i);
                }
                continue
            }
            self.next(i);
        }
        let start = self.pos;
        match self.cur {
            '\0'=>{
                return Ok(TomlTokWithSpan{tok:TomlTok::Eof, span:TomlSpan{start, len:0}})
            }
            ',' => {
                self.next(i);
                return Ok(TomlTokWithSpan{tok:TomlTok::Comma, span:TomlSpan{start, len:1}})
            }
            '[' => {
                self.next(i);
                return Ok(TomlTokWithSpan{tok:TomlTok::BlockOpen, span:TomlSpan{start, len:1}})
            }
            ']' => {
                self.next(i);
                return Ok(TomlTokWithSpan{tok:TomlTok::BlockClose, span:TomlSpan{start, len:1}})
            }
            '{' => {
                self.next(i);
                return Ok(TomlTokWithSpan{tok:TomlTok::ObjectOpen, span:TomlSpan{start, len:1}})
            }
            '}' => {
                self.next(i);
                return Ok(TomlTokWithSpan{tok:TomlTok::ObjectClose, span:TomlSpan{start, len:1}})
            }
            '=' => {
                self.next(i);
                return Ok(TomlTokWithSpan{tok:TomlTok::Equals, span:TomlSpan{start, len:1}})
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
                            return Ok(TomlTokWithSpan{tok:TomlTok::Nan(is_neg), span:TomlSpan{start, len:self.pos - start}})
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
                            return Ok(TomlTokWithSpan{tok:TomlTok::Inf(is_neg), span:TomlSpan{start, len:self.pos - start}})
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
                        return Ok(TomlTokWithSpan{tok:TomlTok::F64(num), span:TomlSpan{start, len:self.pos - start}})
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
                    return Ok(TomlTokWithSpan{tok:TomlTok::Date(num), span:TomlSpan{start, len:self.pos - start}})
                }
                else {
                    if is_neg {
                        if let Ok(num) = num.parse() {
                             return Ok(TomlTokWithSpan{tok:TomlTok::I64(num), span:TomlSpan{start, len:self.pos - start}})
                        }
                        else {
                            return Err(self.err_parse("number"));
                        }
                    }
                    if let Ok(num) = num.parse() {
                        return Ok(TomlTokWithSpan{tok:TomlTok::U64(num), span:TomlSpan{start, len:self.pos - start}})
                    }
                    else {
                        return Err(self.err_parse("number"));
                    }
                }
            },
            'a'..='z' | 'A'..='Z' | '_'  => {
                let mut ident = String::new();
                while self.cur >= 'a' && self.cur <= 'z'
                    || self.cur >= 'A' && self.cur <= 'Z'
                    || self.cur >= '0' && self.cur <= '9'
                    || self.cur == '_' || self.cur == '-' || self.cur == '.'{
                    ident.push(self.cur);
                    self.next(i);
                } 
                if ident == "true" {
                    return Ok(TomlTokWithSpan{tok:TomlTok::Bool(true), span:TomlSpan{start, len:self.pos - start}})
                }
                if ident == "false" {
                    return Ok(TomlTokWithSpan{tok:TomlTok::Bool(false), span:TomlSpan{start, len:self.pos - start}})
                }
                if ident == "inf" {
                    return Ok(TomlTokWithSpan{tok:TomlTok::Inf(false), span:TomlSpan{start, len:self.pos - start}})
                }
                if ident == "nan" {
                    return Ok(TomlTokWithSpan{tok:TomlTok::Nan(false), span:TomlSpan{start, len:self.pos - start}})
                }
                return Ok(TomlTokWithSpan{tok:TomlTok::Ident(ident), span:TomlSpan{start, len:self.pos - start}})
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
                return Ok(TomlTokWithSpan{tok:TomlTok::Str(val), span:TomlSpan{start, len:self.pos - start}})
            },
            _ => {
                return Err(self.err_parse("tokenizer"));
            }
        }
    }
}