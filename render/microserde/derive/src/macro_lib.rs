#![allow(dead_code)]

use proc_macro::{TokenTree, Span, TokenStream, Delimiter, Group, Literal, Ident, Punct, Spacing};
use proc_macro::token_stream::IntoIter;

// little macro utility lib

pub fn error_span(err: &str, span: Span) -> TokenStream {
    let mut tb = TokenBuilder::new();
    tb.ident_with_span("compile_error", span).add("! (").string(err).add(")");
    tb.end()
}

pub fn error(err: &str) -> TokenStream {
    let mut tb = TokenBuilder::new();
    tb.add("compile_error ! (").string(err).add(")");
    tb.end()
}

pub struct TokenBuilder {
    pub groups: Vec<(Delimiter, TokenStream)>
}

impl TokenBuilder {
    pub fn new() -> Self {
        Self {
            groups: vec![(Delimiter::None, TokenStream::new())]
        }
    }
    
    pub fn end(mut self) -> TokenStream {
        if self.groups.len() != 1 {
            panic!("Groups not empty, you missed a pop_group")
        }
        self.groups.pop().unwrap().1
    }
    
    pub fn eprint(&self){
        eprintln!("{}", self.groups.last().unwrap().1.to_string());
    }
    
    pub fn extend(&mut self, tt: TokenTree) -> &mut Self {
        self.groups.last_mut().unwrap().1.extend(Some(tt));
        self
    }
    
    pub fn stream(&mut self, what: Option<TokenStream>) -> &mut Self {
        if let Some(what) = what {
            for c in what.into_iter() {
                self.extend(c);
            }
            self
        }
        else {
            self
        }
    }
    
    pub fn add(&mut self, what: &str) -> &mut Self {
        for part in what.split(" ") {
            match part {
                "{" => self.push_group(Delimiter::Brace),
                "(" => self.push_group(Delimiter::Parenthesis),
                "[" => self.push_group(Delimiter::Bracket),
                "}" => self.pop_group(Delimiter::Brace),
                ")" => self.pop_group(Delimiter::Parenthesis),
                "]" => self.pop_group(Delimiter::Bracket),
                "?" | ";" | "&" | "^" | ":" | "::" | "," | "!" | "." | "<<" | ">>" |
                "->" | "=>" | "<" | ">" | "<=" | ">=" | "=" | "==" | "!=" |
                "+" | "+=" | "-" | "-=" | "*" | "*=" | "/" | "/=" => self.punct(part),
                _ => {
                    if part.len() == 0{
                        continue
                    }
                    match part.chars().next().unwrap(){
                        '0'..='9'=>{
                            self.unsuf_usize(part.parse().expect(&format!("Can't parse number \"{}\"", what))) 
                        },
                        _=>self.ident(part)
                    }
                }
            };
        }
        self
    }
    
    pub fn ident(&mut self, id: &str) -> &mut Self {
        self.extend(TokenTree::from(Ident::new(id, Span::call_site())))
    }
    
    pub fn ident_with_span(&mut self, id: &str, span: Span) -> &mut Self {
        self.extend(TokenTree::from(Ident::new(id, span)))
    }
    
    pub fn punct(&mut self, s: &str) -> &mut Self {
        for (last, c) in s.chars().identify_last() {
            self.extend(TokenTree::from(Punct::new(c, if last {Spacing::Alone} else {Spacing::Joint})));
        }
        self
    }
    
    pub fn string(&mut self, val: &str) -> &mut Self {self.extend(TokenTree::from(Literal::string(val)))}
    pub fn unsuf_usize(&mut self, val: usize) -> &mut Self {self.extend(TokenTree::from(Literal::usize_unsuffixed(val)))}
    pub fn suf_u16(&mut self, val: u16) -> &mut Self {self.extend(TokenTree::from(Literal::u16_suffixed(val)))}
    pub fn chr(&mut self, val:char) -> &mut Self {self.extend(TokenTree::from(Literal::character(val)))}
    pub fn _lit(&mut self, lit: Literal) -> &mut Self {self.extend(TokenTree::from(lit))}
    
    pub fn push_group(&mut self, delim: Delimiter) -> &mut Self {
        self.groups.push((delim, TokenStream::new()));
        self
    }
    
    pub fn stack_as_string(&self)->String{
        let mut ret = String::new();
        for i in (0..self.groups.len() - 1).rev(){
            ret.push_str(&format!("Level {}: {}", i, self.groups[i].1.to_string()));
        }
        ret
    }
    
    pub fn pop_group(&mut self, delim: Delimiter) -> &mut Self {
        if self.groups.len() < 2 {
            eprintln!("Stack dump for error:\n{}", self.stack_as_string());
            panic!("pop_group stack is empty {}", self.groups.len());
        }
        let ts = self.groups.pop().unwrap();
        if ts.0 != delim {
            eprintln!("Stack dump for error:\n{}", self.stack_as_string());
            panic!("pop_group Delimiter mismatch, got {:?} expected {:?}", ts.0, delim);
        }
        self.extend(TokenTree::from(Group::new(delim, ts.1)));
        self
    }
}

pub trait IdentifyLast: Iterator + Sized {
    fn identify_last(self) -> Iter<Self>;
}

impl<It> IdentifyLast for It where It: Iterator {
    fn identify_last(mut self) -> Iter<Self> {
        let e = self.next();
        Iter {
            iter: self,
            buffer: e,
        }
    }
}

pub struct Iter<It> where It: Iterator {
    iter: It,
    buffer: Option<It::Item>,
}

impl<It> Iterator for Iter<It> where It: Iterator {
    type Item = (bool, It::Item);
    
    fn next(&mut self) -> Option<Self::Item> {
        match self.buffer.take() {
            None => None,
            Some(e) => {
                match self.iter.next() {
                    None => Some((true, e)),
                    Some(f) => {
                        self.buffer = Some(f);
                        Some((false, e))
                    },
                }
            },
        }
    }
}

pub struct TokenParser {
    iter_stack: Vec<IntoIter>,
    current: Option<TokenTree>
}

// this parser is optimized for parsing type definitions, not general Rust code

impl TokenParser {
    pub fn new(start: TokenStream) -> Self {
        let mut ret = Self {iter_stack: vec![start.into_iter()], current: None};
        ret.advance();
        ret
    }
    
    pub fn advance(&mut self) {
        let last = self.iter_stack.last_mut().unwrap();
        let value = last.next();
        if let Some(tok) = value {
            self.current = Some(tok);
        }
        else {
            self.current = None;
        }
    }
    
    pub fn unexpected(&self) -> TokenStream {
        error("Unexpected token")
    }
    
    pub fn is_delim(&mut self, delim: Delimiter) -> bool {
        if let Some(TokenTree::Group(group)) = &self.current {
            group.delimiter() == delim
        }
        else {
            false
        }
    }
    
    pub fn is_brace(&mut self) -> bool {
        self.is_delim(Delimiter::Brace)
    }
    
    pub fn is_paren(&mut self) -> bool {
        self.is_delim(Delimiter::Parenthesis)
    }
    
    pub fn is_bracket(&mut self) -> bool {
        self.is_delim(Delimiter::Bracket)
    }
    
    pub fn open_delim(&mut self, delim: Delimiter) -> bool {
        if let Some(TokenTree::Group(group)) = &self.current {
            if group.delimiter() == delim {
                self.iter_stack.push(group.stream().into_iter());
                self.advance();
                return true
            }
        }
        false
    }
    
    pub fn open_brace(&mut self) -> bool {
        self.open_delim(Delimiter::Brace)
    }
    
    pub fn open_paren(&mut self) -> bool {
        self.open_delim(Delimiter::Parenthesis)
    }
    
    pub fn open_bracket(&mut self) -> bool {
        self.open_delim(Delimiter::Bracket)
    }
    
    pub fn is_eot(&mut self) -> bool{
        if self.current.is_none() && self.iter_stack.len() != 0 {
            return true
        }
        else{
            return false
        }
    }
    
    pub fn eat_eot(&mut self) -> bool {
        // current is None
        if self.is_eot() {
            self.iter_stack.pop();
            if self.iter_stack.len() != 0 {
                self.advance()
            }
            return true;
        }
        return false
    }
    
    pub fn eat_ident(&mut self, what: &str) -> bool {
        // check if our current thing is an ident, ifso eat it.
        if let Some(TokenTree::Ident(ident)) = &self.current {
            if ident.to_string() == what {
                self.advance();
                return true
            }
        }
        return false
    }
    
    pub fn is_punct(&mut self, what: char) -> bool {
        // check if our punct is multichar.
        if let Some(TokenTree::Punct(current)) = &self.current {
            if current.as_char() == what && (what == '>' || current.spacing() == Spacing::Alone){
                return true
            }
            else {
                return false
            }
        }
        else {
            return false
        }
    }
    
    pub fn eat_punct(&mut self, what: char) -> bool {
        if self.is_punct(what) {
            self.advance();
            return true
        }
        false
    }
    
    pub fn eat_any_ident(&mut self) -> Option<String> {
        if let Some(TokenTree::Ident(ident)) = &self.current {
            let ret = Some(ident.to_string());
            self.advance();
            return ret
        }
        return None
    }
    
    pub fn eat_where_clause(&mut self, add_where: Option<&str>) -> Option<TokenStream> {
        let mut tb = TokenBuilder::new();
        if self.eat_ident("where") {
            tb.add("where");
            // ok now we parse an ident
            loop {
                if let Some(ident) = self.eat_any_ident() {
                    tb.ident(&ident);
                    tb.stream(self.eat_generic());
                    
                    if !self.eat_punct(':') {
                        return None
                    }
                    tb.add(":");
                    loop {
                        if let Some(ident) = self.eat_any_ident() {
                            tb.add(&ident);
                            tb.stream(self.eat_generic());
                            // check if we have upnext
                            // {, + or ,
                            if self.eat_punct('+') {
                                tb.add("+");
                                continue
                            }
                            if self.eat_punct(',') { // next one
                                if let Some(add_where) = add_where {
                                    tb.add("+");
                                    tb.ident(add_where);
                                }
                                tb.add(",");
                                break
                            }
                            if self.is_brace() || self.is_punct(';') { // upnext is a brace.. we're done
                                if let Some(add_where) = add_where {
                                    tb.add("+");
                                    tb.ident(add_where);
                                }
                                return Some(tb.end())
                            }
                        }
                        else {
                            return None // unexpected
                        }
                    }
                }
                else {
                    return None // unexpected
                }
            }
        }
        return None
    }
    
    pub fn eat_struct_field(&mut self) -> Option<(String, TokenStream)> {
        // letsparse an ident
        if let Some(field) = self.eat_any_ident() {
            if self.eat_punct(':') {
                if let Some(ty) = self.eat_type() {
                    return Some((field, ty))
                }
            }
        }
        return None
    }
    
    pub fn eat_all_struct_fields(&mut self, )->Option<Vec<(String, TokenStream)>>{
        
        if self.open_brace(){
            let mut fields = Vec::new();
            while !self.eat_eot(){
                if let Some((field, ty)) = self.eat_struct_field(){
                    fields.push((field, ty));
                    self.eat_punct(',');
                }
                else{
                    return None
                }
            }
            return Some(fields)
        }
        return None
    }
    

    pub fn eat_generic(&mut self) -> Option<TokenStream> {
        let mut tb = TokenBuilder::new();
        // if we have a <, keep running and keep a < stack
        
        if self.eat_punct('<') {
            tb.add("<");
            let mut stack = 1;
            // keep eating things till we are at stack 0 for a ">"
            while stack > 0 {
                if self.eat_punct('<') {
                    tb.add("<");
                    stack += 1;
                }
                if self.eat_punct('>') {
                    tb.add(">");
                    stack -= 1;
                }
                else if self.eat_eot() { // shits broken
                    return None
                }
                else { // store info here in generics struct
                    if let Some(current) = &self.current {
                        tb.extend(current.clone());
                    }
                    self.advance();
                }
            }
            
            return Some(tb.end())
        }
        return None
    }
    
    pub fn eat_all_types(&mut self)->Option<Vec<TokenStream>>{
        if self.open_paren(){
            let mut ret = Vec::new();
            while !self.eat_eot(){
                if let Some(tt) = self.eat_type(){
                    ret.push(tt);
                    self.eat_punct(',');
                }
                else{
                    return None
                }
            }
            Some(ret)
        }
        else{
            None
        }
    }
    
    pub fn eat_type(&mut self) -> Option<TokenStream> {
        let mut tb = TokenBuilder::new();
        if self.open_bracket(){ // array type
            tb.add("[");
            while !self.eat_eot(){
                if let Some(current) = &self.current {
                    tb.extend(current.clone());
                }
                self.advance();
            }
            tb.add("]");
            return Some(tb.end())
        }
        else if let Some(ty) = self.eat_any_ident() {
            tb.ident(&ty);
            tb.stream(self.eat_generic());

            return Some(tb.end())
        }
        return None
    }
}
