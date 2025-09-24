// Makepad script streaming tokenizer

use crate::id::Id;
use crate::colorhex::hex_bytes_to_u32;
use makepad_script_derive::*;
//use crate::value::Value;

#[derive(Copy,Clone)]
pub enum ScriptToken{
    End,
    StreamEnd,
    Identifier(Id),
    Operator(Id),
    OpenCurly,
    CloseCurly,
    OpenRound,
    CloseRound,
    OpenSquare,
    CloseSquare,
    StringUnfinished(usize),
    String(usize),
    Number(f64),
    Color(u32),
}

impl ScriptToken{
    pub fn is_identifier(&self)->bool{match self{ScriptToken::Identifier(_)=>true,_=>false}}
    pub fn identifier(&self)->Id{match self{ScriptToken::Identifier(id)=>*id,_=>id!()}}
    pub fn operator(&self)->Id{match self{ScriptToken::Operator(id)=>*id,_=>id!()}}
    pub fn is_operator(&self)->bool{match self{ScriptToken::Operator(_)=>true,_=>false}}
    pub fn is_open_curly(&self)->bool{match self{ScriptToken::OpenCurly=>true,_=>false}}
    pub fn is_close_curly(&self)->bool{match self{ScriptToken::CloseCurly=>true,_=>false}}
    pub fn is_open_round(&self)->bool{match self{ScriptToken::OpenRound=>true,_=>false}}
    pub fn is_close_round(&self)->bool{match self{ScriptToken::CloseRound=>true,_=>false}}
    pub fn is_open_square(&self)->bool{match self{ScriptToken::OpenSquare=>true,_=>false}}
    pub fn is_close_square(&self)->bool{match self{ScriptToken::CloseSquare=>true,_=>false}}
    pub fn is_string(&self)->bool{match self{ScriptToken::StringUnfinished(_)|ScriptToken::String(_)=>true,_=>false}}
    pub fn is_number(&self)->bool{match self{ScriptToken::Number(_)=>true,_=>false}}
    pub fn is_color(&self)->bool{match self{ScriptToken::Color(_)=>true,_=>false}}
}

pub struct ScriptTokenPos{
    pub token: ScriptToken,
    pos: usize
}

#[derive(Default)]
enum State{ 
    #[default]
    Whitespace,
    Identifier,
    Operator,
    String(bool),
    EscapeInString(bool),
    UnicodeHexInString(bool),
    UnicodeCurlyInString(bool),
    AsciiHexInString(bool),
    BlockComment(usize),
    MaybeEndBlock(usize),
    LineComment,
    Number,
    Color
}

pub struct ScriptDoc{
    pos: usize,
    pub tokens: Vec<ScriptTokenPos>,
    original: String,
    strings: String,
    string_table: Vec<(usize, usize)>,
    temp: String,
    state: State,
}

impl Default for ScriptDoc{
    fn default()->Self{
        Self{
            tokens: Default::default(),
            temp: Default::default(),
            original: Default::default(),
            strings: Default::default(),
            string_table:vec![(0,0)],
            state: State::Whitespace,
            pos: 0,
        }
    }
}

pub struct ScriptLoc{
    pub row: usize,
    pub col: usize
}

impl ScriptDoc{
    pub fn pos_to_loc(&self, pos:usize)->Option<ScriptLoc>{
        let mut row = 0;
        let mut col = 0;
        for (i, c) in self.original.chars().enumerate(){
            if c == '\n'{
                row +=1;
                col = 0;
            }
            else{
                col += 1;
            }
            if i >= pos{
                return Some(ScriptLoc{row, col})
            }
        }
        None
    }
    
    fn emit_number(&mut self){
        let number = if let Ok(v) = self.temp.parse::<f64>(){
            self.temp.clear();
            v
        }
        else{
            0.0
        };
        let len = self.temp.len();
        self.temp.clear();
        self.tokens.push(ScriptTokenPos{
            pos: self.pos - len,
            token: ScriptToken::Number(number)
        });
    }
    
    fn emit_identifier(&mut self){
        let id = match Id::from_str_with_lut(&self.temp){
            Err(str)=>{
                println!("--WARNING-- Id LUT collision between {} and {}", self.temp, str);
                Id::from_str(&self.temp)
            }
            Ok(id)=>{
                id
            }
        };
        let len = self.temp.len();
        self.temp.clear();
        self.tokens.push(ScriptTokenPos{
            pos: self.pos - len,
            token: ScriptToken::Identifier(id)
        });
    }
    
    fn emit_operator(&mut self){
        let id = match Id::from_str_with_lut(&self.temp){
            Err(str)=>{
                println!("--WARNING-- Id LUT collision between {} and {}", self.temp, str);
                Id::from_str(&self.temp)
            }
            Ok(id)=>{
                id
            }
        };
        let len = self.temp.len();
        self.temp.clear();
        self.tokens.push(ScriptTokenPos{
            pos: self.pos - len,
            token: ScriptToken::Operator(id)
        });
    }
    
    fn emit_color(&mut self){
        let color = match hex_bytes_to_u32(&self.temp.as_bytes()){
            Err(())=>{
                0xff00ffff
            }
            Ok(color)=>{
                color
            }
        };
        let len = self.temp.len();
        self.temp.clear();
        self.tokens.push(ScriptTokenPos{
            pos: self.pos - len,
            token: ScriptToken::Color(color)
        });
    }
            
        
    fn emit_token_here(&mut self, token: ScriptToken){
        self.tokens.push(ScriptTokenPos{
            pos: self.pos,
            token
        })
    }
    
    fn append_unfinished_string(&mut self, c:char){
        if let Some(ScriptTokenPos{token:ScriptToken::StringUnfinished(index),..}) = self.tokens.last_mut(){
            self.strings.push(c);
            self.string_table[*index].1 += 1;
        }
        else{
            self.tokens.push(ScriptTokenPos{pos: self.pos, token: ScriptToken::StringUnfinished(self.string_table.len())});
            self.string_table.push((self.strings.len(),1));
            self.strings.push(c);
        }
    }
    
    fn finish_string(&mut self){
        if let Some(ScriptTokenPos{token:ScriptToken::StringUnfinished(_),..}) = self.tokens.last(){
            if let Some(ScriptTokenPos{token:ScriptToken::StringUnfinished(index),pos}) = self.tokens.pop(){
                self.tokens.push(ScriptTokenPos{token:ScriptToken::String(index), pos})
            }
        }
        else{
            self.tokens.push(ScriptTokenPos{token:ScriptToken::String(0), pos:self.pos})
        }
    }
    
    pub fn parse(&mut self, new_chars: &str)->&[ScriptTokenPos]{
        let mut iter = new_chars.chars();
        
        fn is_operator(c:char)->bool{
            c == '!' || c == '^' || c == '&' || c == '*' || c == '+' || c == '-'|| c == '|' || c == '?' || c == ':' || c == '=' || c == '@' || c=='>' || c=='<' || c == '.'
        }
        fn is_block(c:char)->Option<ScriptToken>{
            match c{
                '{'=>Some(ScriptToken::OpenCurly),
                '}'=>Some(ScriptToken::CloseCurly),
                '['=>Some(ScriptToken::OpenSquare),
                ']'=>Some(ScriptToken::CloseSquare),
                '('=>Some(ScriptToken::OpenRound),
                ')'=>Some(ScriptToken::CloseRound),
                _=>None
            }
        }
        // unfinished string at the end
        let start = if let Some(ScriptTokenPos{token:ScriptToken::StringUnfinished(_),..}) = self.tokens.last_mut(){
            self.tokens.len() - 1
        }
        else{
            self.tokens.len()
        };
        
        while let Some(c) = iter.next(){
            self.original.push(c);
            self.pos += 1;
            match self.state{
                State::Whitespace=>{
                    if c.is_numeric(){
                        self.state = State::Number;
                        self.temp.push(c);
                    }
                    else if c == '_' || c == '$' || c.is_alphabetic(){
                        self.state = State::Identifier;
                        self.temp.push(c);
                    }
                    else if c == '#'{
                        self.state = State::Color;
                    }
                    else if is_operator(c){
                        self.state = State::Operator;
                        self.temp.push(c);
                    }
                    else if c == '"'{
                        self.state = State::String(true);
                    }
                    else if c == '\''{
                        self.state = State::String(false);
                    }
                    else if let Some(tok) = is_block(c){
                        self.emit_token_here(tok);
                    }
                }
                State::Identifier=>{
                    if c == '_' || c == '$' || c.is_alphanumeric(){
                        self.temp.push(c);
                    }
                    else if c.is_whitespace(){
                        self.emit_identifier();
                        self.state = State::Whitespace;
                    }
                    else if is_operator(c){
                        self.emit_identifier();
                        self.state = State::Operator;
                        self.temp.push(c);
                    }
                    else if c == '#'{
                        self.emit_identifier();
                        self.state = State::Color;
                    }
                    else if let Some(tok) = is_block(c){
                        self.emit_identifier();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    }
                    else if c == '"'{
                        self.emit_identifier();
                        self.state = State::String(true);
                    }
                    else if c == '\''{
                        self.emit_identifier();
                        self.state = State::String(false);
                    }
                    else{
                        self.emit_identifier();
                        self.state = State::Whitespace;
                    }
                }
                State::Operator=>{
                    // detect comments
                    if c.is_whitespace(){
                        self.emit_operator();
                        self.state = State::Whitespace;
                    }
                    else if c.is_numeric(){
                        self.emit_operator();
                        self.state = State::Number;
                        self.temp.push(c);
                    }
                    else if c == '_' || c == '$' || c.is_alphabetic(){
                        self.emit_operator();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    }
                    else if c == '/' && self.temp.chars().last() == Some('/'){
                        self.temp.pop();
                        if self.temp.len()>0{
                            self.emit_operator();
                        }
                        self.state = State::LineComment;
                    }
                    else if c == '*' && self.temp.chars().last() == Some('/'){
                        self.temp.pop();
                        if self.temp.len()>0{
                            self.emit_operator();
                        }
                        self.state = State::BlockComment(0);
                    }
                    else if c == '-' && self.temp.len() > 0 && self.temp.chars().last() != Some('-'){
                        self.emit_operator();
                        self.temp.push(c);
                    }
                    else if c == '+' && self.temp.len() > 0 && self.temp.chars().last() != Some('+'){
                        self.emit_operator();
                        self.temp.push(c);
                    }
                    else if (c == '!' || c == '^') && self.temp.len() > 0{
                        self.emit_operator();
                        self.temp.push(c);
                    }
                    else if c == '#'{
                        self.emit_operator();
                        self.state = State::Color;
                    }
                    else if is_operator(c){
                        self.temp.push(c);
                    }
                    else if c == '"'{
                        self.emit_operator();
                        self.state = State::String(true);
                    }
                    else if c == '\''{
                        self.emit_operator();
                        self.state = State::String(false);
                    }
                    else if let Some(tok) = is_block(c){
                        self.emit_operator();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    }
                    else{
                        self.emit_operator();
                        self.state = State::Whitespace;
                    }
                }
                State::EscapeInString(double)=>{
                    // ok lets see what we have for an escape character sequence
                    if c == '\\'{
                        self.append_unfinished_string('\\');
                        self.state = State::String(double);
                    }
                    else if c == 'r'{
                        self.append_unfinished_string('\r');
                        self.state = State::String(double);
                    }
                    else if c == 'n'{
                        self.append_unfinished_string('\n');
                        self.state = State::String(double);
                    }
                    else if c == 't'{
                        self.append_unfinished_string('\t');
                        self.state = State::String(double);
                    }
                    else if c == '0'{
                        self.append_unfinished_string('\0');
                        self.state = State::String(double);
                    }
                    else if c == 'x'{
                        self.state = State::AsciiHexInString(double);
                    }
                    else if c == 'u'{
                        self.state = State::UnicodeHexInString(double);
                    }
                }
                State::AsciiHexInString(double)=>{
                    self.temp.push(c);
                    if self.temp.len() == 2{
                        if let Ok(v) = i64::from_str_radix(&self.temp, 16){
                            self.append_unfinished_string(v as u8 as char);                            
                        }
                        self.temp.clear();
                        self.state = State::String(double);
                    }
                }
                State::UnicodeHexInString(double)=>{
                    if c == '{'{
                        self.state = State::UnicodeCurlyInString(double);
                    }
                    else{ // its kinda unknown how long we need to keep pushing this
                        self.temp.push(c);
                        if self.temp.len() == 4{
                            if let Ok(v) = i64::from_str_radix(&self.temp, 16){
                                if let Some(v) = char::from_u32(v as u32){
                                    self.append_unfinished_string(v);                            
                                }
                            }
                            self.temp.clear();
                            self.state = State::String(double);
                        }
                    }
                }
                State::UnicodeCurlyInString(double)=>{
                    if c == '}'{
                        if let Ok(v) = i64::from_str_radix(&self.temp, 16){
                            if let Some(v) = char::from_u32(v as u32){
                                self.append_unfinished_string(v);                            
                            }
                        }
                        self.temp.clear();
                        self.state = State::String(double);
                    }
                    else{
                        self.temp.push(c);
                    }
                }
                State::String(double)=>{
                    // check last token is 
                    if c == '\\'{ // escape char 
                        self.temp.clear();
                        self.state = State::EscapeInString(double);
                    }
                    else if (double && c == '"') || (!double && c == '\''){
                        self.finish_string();
                        self.state = State::Whitespace;
                    }
                    else{
                        self.append_unfinished_string(c);
                    }
                }
                State::BlockComment(depth)=>{
                    if c == '*'{ // end block comment
                        self.state = State::MaybeEndBlock(depth);
                    }
                }
                State::MaybeEndBlock(depth)=>{
                    if c == '/'{ // end block comment
                        if depth > 0{
                            self.state = State::BlockComment(depth - 1)
                        }
                        else{
                            self.state = State::Whitespace;
                        }
                    }
                    else{
                        self.state = State::BlockComment(depth)
                    }
                }
                State::LineComment=>{
                    if c == '\n'{ // end line comment
                        self.state = State::Whitespace;
                    }
                }
                State::Number=>{
                    if c.is_numeric(){
                        self.temp.push(c);    
                    }
                    else if c == '.' && self.temp.chars().position(|v| v == '.').is_none(){
                        self.temp.push(c);    
                    }
                    else if (c == 'e' || c == 'E') && self.temp.chars().position(|v| v == 'e' ||  v == 'E').is_none(){
                        self.temp.push(c);    
                    }
                    else if (c == 'x' || c == 'X') && self.temp.chars().position(|v| v == 'x' ||  v == 'X').is_none(){
                        self.temp.push(c);    
                    }
                    else if c == '_'{ // skip these
                    }
                    else if c.is_alphabetic(){
                        self.emit_number();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    }
                    else if c == '#'{
                        self.emit_number();
                        self.state = State::Color;
                        self.temp.push(c);
                    }
                    else if is_operator(c){
                        self.emit_number();
                        self.state = State::Operator;
                        self.temp.push(c);
                    }
                    else if c == '"'{
                        self.emit_number();
                        self.state = State::String(true);
                    }
                    else if c == '\''{ 
                        self.emit_number();
                        self.state = State::String(false);
                    }
                    else if let Some(tok) = is_block(c){
                        self.emit_number();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    }
                    else{
                        self.emit_number();
                        self.state = State::Whitespace;
                    }
                }
                State::Color=>{
                    if c>='0' && c<='9' || c>='a' && c<='f' || c>='A' && c<='F'{
                        self.temp.push(c);    
                        if  self.temp.len() == 8{
                            self.emit_color();
                            self.state = State::Whitespace
                        }
                    }
                    else if c == 'x' && self.temp.len() == 0{ // eat first x
                    }
                    else if c.is_alphabetic(){
                        self.emit_color();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    }
                    else if c == '#'{
                        self.emit_color();
                        self.state = State::Color;
                        self.temp.push(c);
                    }
                    else if is_operator(c){
                        self.emit_color();
                        self.state = State::Operator;
                        self.temp.push(c);
                    }
                    else if c == '"'{
                        self.emit_color();
                        self.state = State::String(true);
                    }
                    else if c == '\''{
                        self.emit_color();
                        self.state = State::String(false);
                    }
                    else if let Some(tok) = is_block(c){
                        self.emit_color();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    }
                    else{
                        self.emit_color();
                        self.state = State::Whitespace;
                    }
                }
            }
        }
        &self.tokens[start..self.tokens.len()]
    }
}
