#![allow(dead_code)]
extern crate proc_macro;
use proc_macro::{TokenTree, Span, TokenStream, Delimiter, Group, Literal, Ident, Punct, Spacing};
use proc_macro::token_stream::IntoIter;

// little macro utility lib

pub fn error_span(err: &str, span: Span) -> TokenStream {
    let mut tb = TokenBuilder::new();
    tb.ident_with_span("compile_error", span).add("! (").string(err).add(") ;");
    tb.end()
}

pub fn error(err: &str) -> TokenStream {
    let mut tb = TokenBuilder::new();
    tb.add("compile_error ! (").string(err).add(") ;");
    tb.end()
}

pub fn error_result(err: &str) -> Result<(), TokenStream> {
    let mut tb = TokenBuilder::new();
    tb.add("compile_error ! (").string(err).add(") ;");
    Err(tb.end())
}

pub fn unwrap_option(input: TokenStream) -> Result<TokenStream, TokenStream> {
    let mut ty_parser = TokenParser::new(input.clone());
    if ty_parser.eat_ident("Option") {
        if !ty_parser.eat_punct_alone('<') {
            panic!()
        }
        Ok(ty_parser.eat_level_or_punct('>'))
    }
    else {
        Err(input)
    }
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
    
    pub fn is_empty(&self) -> bool {
        self.groups.len() == 1 && self.groups[0].1.is_empty()
    }
    
    pub fn end(mut self) -> TokenStream {
        if self.groups.len() != 1 {
            panic!("Groups not empty, you missed a pop_group")
        }
        self.groups.pop().unwrap().1
    }
    
    pub fn eprint(&self) {
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
        let b = what.as_bytes();
        let mut o = 0;
        while o < b.len() {
            let c0 = b[o] as char;
            let c1 = if o + 1 < b.len() {b[o + 1] as char}else {'\0'};
            match (c0, c1) {
                ('\r', _) | ('\n', _) | (' ', _) | ('\t', _) => {o += 1;}
                ('{', _) => {self.push_group(Delimiter::Brace); o += 1;},
                ('(', _) => {self.push_group(Delimiter::Parenthesis); o += 1;},
                ('[', _) => {self.push_group(Delimiter::Bracket); o += 1;},
                ('}', _) => {self.pop_group(Delimiter::Brace); o += 1;},
                (')', _) => {self.pop_group(Delimiter::Parenthesis); o += 1;},
                (']', _) => {self.pop_group(Delimiter::Bracket); o += 1;},
                ('<', '<') | ('>', '>') | ('&', '&') | ('|', '|') |
                ('-', '>') | ('=', '>') |
                ('<', '=') | ('>', '=') | ('=', '=') | ('!', '=') | (':', ':') |
                ('+', '=') | ('-', '=') | ('*', '=') | ('/', '=') | ('.', '.') => {
                    self.punct(std::str::from_utf8(&b[o..o + 2]).unwrap());
                    o += 2;
                }
                ('+', _) | ('-', _) | ('*', _) | ('/', _) | ('#', _) |
                ('=', _) | ('<', _) | ('>', _) | ('?', _) | (';', _) | ('&', _) |
                ('^', _) | (':', _) | (',', _) | ('!', _) | ('.', _) | ('|', _) => {
                    self.punct(std::str::from_utf8(&b[o..o + 1]).unwrap());
                    o += 1;
                },
                ('0', 'x') => { // this needs to be fancier but whatever.
                    let mut e = o + 2;
                    let mut out: u64 = 0;
                    while e < b.len() {
                        match b[e] {
                            b'0'..=b'9' => out = (out << 4) | (b[e] - b'0') as u64,
                            b'a'..=b'f' => out = (out << 4) | (b[e] - b'a' + 10) as u64,
                            b'A'..=b'F' => out = (out << 4) | (b[e] - b'A' + 10) as u64,
                            b'_' => (),
                            _ => break,
                        };
                        e += 1;
                    }
                    self.suf_u64(out);
                    o = e;
                }
                ('0'..='9', _) => {
                    let mut e = o + 1;
                    while e < b.len() {
                        match b[e] {
                            b'0'..=b'9' => e += 1,
                            _ => break,
                        }
                    }
                    let num = std::str::from_utf8(&b[o..e]).unwrap();
                    self.unsuf_usize(num.parse().unwrap_or_else(|_| panic!("Can't parse usize number \"{}\"", what)));
                    o = e;
                }
                ('"', _) => {
                    let mut e = o + 1;
                    while e < b.len() {
                        match b[e] {
                            b'"' => break,
                            _ => e += 1,
                        }
                    }
                    self.string(std::str::from_utf8(&b[o + 1..e]).unwrap());
                    o = e + 1;
                }
                ('\'', _) => {
                    let mut e = o + 1;
                    while e < b.len() {
                        match b[e] {
                            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_' => e += 1,
                            _ => break,
                        }
                    }
                    if o == e {
                        panic!("Unexpected character {:?}", b[e] as char);
                    }
                    let ident = std::str::from_utf8(&b[o + 1..e]).unwrap();
                    self.lifetime_mark();
                    self.ident(ident);
                    o = e;
                }
                _ => {
                    let mut e = o;
                    while e < b.len() {
                        match b[e] {
                            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_' => e += 1,
                            _ => break,
                        }
                    }
                    if o == e {
                        panic!("Unexpected character {:?}", b[e] as char);
                    }
                    let ident = std::str::from_utf8(&b[o..e]).unwrap();
                    self.ident(ident);
                    o = e;
                }
            }
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
    
    pub fn lifetime_mark(&mut self) -> &mut Self {
        self.extend(TokenTree::from(Punct::new('\'', Spacing::Joint)));
        self
    }
    
    pub fn sep(&mut self) -> &mut Self {
        self.extend(TokenTree::from(Punct::new(':', Spacing::Joint)));
        self.extend(TokenTree::from(Punct::new(':', Spacing::Alone)));
        self
    }
    
    pub fn string(&mut self, val: &str) -> &mut Self {self.extend(TokenTree::from(Literal::string(val)))}
    pub fn unsuf_usize(&mut self, val: usize) -> &mut Self {self.extend(TokenTree::from(Literal::usize_unsuffixed(val)))}
    pub fn suf_u16(&mut self, val: u16) -> &mut Self {self.extend(TokenTree::from(Literal::u16_suffixed(val)))}
    pub fn suf_u32(&mut self, val: u32) -> &mut Self {self.extend(TokenTree::from(Literal::u32_suffixed(val)))}
    pub fn suf_u64(&mut self, val: u64) -> &mut Self {self.extend(TokenTree::from(Literal::u64_suffixed(val)))}
    pub fn unsuf_f32(&mut self, val: f32) -> &mut Self {self.extend(TokenTree::from(Literal::f32_unsuffixed(val)))}
    pub fn unsuf_f64(&mut self, val: f64) -> &mut Self {self.extend(TokenTree::from(Literal::f64_unsuffixed(val)))}
    pub fn unsuf_i64(&mut self, val: i64) -> &mut Self {self.extend(TokenTree::from(Literal::i64_unsuffixed(val)))}
    
    pub fn chr(&mut self, val: char) -> &mut Self {self.extend(TokenTree::from(Literal::character(val)))}
    pub fn _lit(&mut self, lit: Literal) -> &mut Self {self.extend(TokenTree::from(lit))}
    
    pub fn push_group(&mut self, delim: Delimiter) -> &mut Self {
        self.groups.push((delim, TokenStream::new()));
        self
    }
    
    pub fn stack_as_string(&self) -> String {
        let mut ret = String::new();
        for i in (0..self.groups.len() - 1).rev() {
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

impl Default for TokenBuilder {
    fn default() -> Self {
        Self::new()
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

#[derive(Debug)]
pub struct Attribute {
    pub name: String,
    pub args: Option<TokenStream>
}

pub struct StructField {
    pub name: String,
    pub ty: TokenStream,
    pub attrs: Vec<Attribute>
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
    pub current: Option<TokenTree>
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
        // skip over ///
        
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
    
    pub fn is_group_with_delim(&mut self, delim: Delimiter) -> bool {
        if let Some(TokenTree::Group(group)) = &self.current {
            delim == group.delimiter()
        }
        else {
            false
        }
    }
    
    pub fn open_group(&mut self) -> Option<Delimiter> {
        if let Some(TokenTree::Group(group)) = &self.current {
            let delim = group.delimiter();
            self.iter_stack.push(group.stream().into_iter());
            self.advance();
            return Some(delim)
        }
        None
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
    
    pub fn is_eot(&mut self) -> bool {
        self.current.is_none() && !self.iter_stack.is_empty()
    }
    
    pub fn eat_eot(&mut self) -> bool {
        // current is None
        if self.is_eot() {
            self.iter_stack.pop();
            if !self.iter_stack.is_empty() {
                self.advance()
            }
            return true;
        }
        false
    }
    
    pub fn eat_level(&mut self) -> TokenStream {
        let mut tb = TokenBuilder::new();
        while !self.eat_eot() {
            tb.extend(self.current.clone().unwrap());
            self.advance();
        }
        tb.end()
    }
    
    pub fn eat_level_or_punct(&mut self, what: char) -> TokenStream {
        let mut tb = TokenBuilder::new();
        while !self.eat_eot() {
            if self.is_punct_alone(what) {
                self.advance();
                return tb.end();
            }
            tb.extend(self.current.clone().unwrap());
            self.advance();
        }
        tb.end()
    }
    
    pub fn eat_ident(&mut self, what: &str) -> bool {
        // check if our current thing is an ident, ifso eat it.
        if let Some(TokenTree::Ident(ident)) = &self.current {
            if ident.to_string() == what {
                self.advance();
                return true
            }
        }
        false
    }
    
    pub fn is_literal(&mut self) -> bool {
        // check if our current thing is an ident, ifso eat it.
        if let Some(TokenTree::Literal(_)) = &self.current {
            return true
        }
        false
    }
    
    pub fn eat_literal(&mut self) -> Option<Literal> {
        // check if our current thing is an ident, ifso eat it.
        if let Some(TokenTree::Literal(lit)) = &self.current {
            let ret = Some(lit.clone());
            self.advance();
            return ret
        }
        None
    }
    
    pub fn span(&self) -> Option<Span> {
        self.current.as_ref().map(|current| current.span())
    }
    
    pub fn is_punct_alone(&mut self, what: char) -> bool {
        // check if our punct is multichar.
        if let Some(TokenTree::Punct(current)) = &self.current {
            if current.as_char() == what && (current.as_char() == '>' || current.spacing() == Spacing::Alone) {
                return true
            }
        }
        false
    }
    
    pub fn is_punct_any(&mut self, what: char) -> bool {
        // check if our punct is multichar.
        if let Some(TokenTree::Punct(current)) = &self.current {
            if current.as_char() == what {
                return true
            }
        }
        false
    }
    
    
    pub fn eat_double_colon_destruct(&mut self) -> bool {
        // check if our punct is multichar.
        if let Some(TokenTree::Punct(current)) = &self.current {
            if current.as_char() == ':' && current.spacing() == Spacing::Joint {
                self.advance();
                if let Some(TokenTree::Punct(current)) = &self.current {
                    if current.as_char() == ':' && current.spacing() == Spacing::Alone {
                        self.advance();
                        return true
                    }
                }
            }
        }
        false
    }
    
    pub fn eat_sep(&mut self) -> bool {
        // check if our punct is multichar.
        if let Some(TokenTree::Punct(current)) = &self.current {
            if current.as_char() == ':' && current.spacing() == Spacing::Joint {
                self.advance();
                if let Some(TokenTree::Punct(current)) = &self.current {
                    if current.as_char() == ':' && current.spacing() == Spacing::Alone {
                        self.advance();
                        return true
                    }
                }
            }
        }
        false
    }
    
    pub fn eat_punct_alone(&mut self, what: char) -> bool {
        if self.is_punct_alone(what) {
            self.advance();
            return true
        }
        false
    }
    
    pub fn eat_punct_any(&mut self, what: char) -> bool {
        if self.is_punct_any(what) {
            self.advance();
            return true
        }
        false
    }
    
    pub fn eat_any_punct(&mut self) -> Option<String> {
        let mut out = String::new();
        while let Some(TokenTree::Punct(current)) = &self.current {
            out.push(current.as_char());
            if current.spacing() == Spacing::Alone {
                self.advance();
                return Some(out);
            }
            self.advance();
        }
        None
    }
    
    pub fn eat_any_ident(&mut self) -> Option<String> {
        if let Some(TokenTree::Ident(ident)) = &self.current {
            let ret = Some(ident.to_string());
            self.advance();
            return ret
        }
        None
    }
    
    pub fn eat_any_ident_with_span(&mut self) -> Option<(String, Span)> {
        if let Some(TokenTree::Ident(ident)) = &self.current {
            let ret = Some((ident.to_string(), self.span().unwrap()));
            self.advance();
            return ret
        }
        None
    }
    
    pub fn expect_any_ident(&mut self) -> Result<String, TokenStream> {
        if let Some(TokenTree::Ident(ident)) = &self.current {
            let ret = ident.to_string();
            self.advance();
            return Ok(ret)
        }
        Err(error("Expected any ident"))
    }
    
    
    pub fn expect_punct_alone(&mut self, what: char) -> Result<(), TokenStream> {
        if self.is_punct_alone(what) {
            self.advance();
            Ok(())
        }
        else {
            Err(error(&format!("Expected punct {}", what)))
        }
    }
    
    pub fn expect_punct_any(&mut self, what: char) -> Result<(), TokenStream> {
        if self.is_punct_any(what) {
            self.advance();
            Ok(())
        }
        else {
            Err(error(&format!("Expected punct {}", what)))
        }
    }
    
    pub fn eat_ident_path(&mut self) -> Option<TokenStream> {
        let mut tb = TokenBuilder::new();
        while let Some(ident) = self.eat_any_ident() {
            tb.ident(&ident);
            if !self.eat_sep() {
                break
            }
            tb.sep();
        }

        let ts = tb.end();
        if !ts.is_empty() {
            return Some(ts)
        }
        None
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
                    
                    if !self.eat_punct_alone(':') {
                        return None
                    }
                    tb.add(":");
                    loop {
                        if let Some(ident) = self.eat_any_ident() {
                            tb.add(&ident);
                            tb.stream(self.eat_generic());
                            // check if we have upnext
                            // {, + or ,
                            if self.eat_punct_alone('+') {
                                tb.add("+");
                                continue
                            }
                            if self.eat_punct_alone(',') { // next one
                                if let Some(add_where) = add_where {
                                    tb.add("+");
                                    tb.ident(add_where);
                                }
                                tb.add(",");
                                break
                            }
                            if self.is_brace() || self.is_punct_alone(';') { // upnext is a brace.. we're done
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
        None
    }
    
    pub fn eat_struct_field(&mut self) -> Option<StructField> {
        // letsparse an ident
        let attrs = self.eat_attributes();
        
        self.eat_ident("pub");
        if let Some(field) = self.eat_any_ident() {
            if self.eat_punct_alone(':') {
                if let Some(ty) = self.eat_type() {
                    return Some(StructField {name: field, ty, attrs})
                }
            }
        }
        None
    }
    
    pub fn eat_attributes(&mut self) -> Vec<Attribute> {
        let mut results = Vec::new();
        while self.eat_punct_alone('#') { // parse our attribute
            if !self.open_bracket() {
                break;
            }
            let mut assign_form = false;
            while let Some(ident) = self.eat_any_ident() {
                // we might have an =
                if self.eat_punct_alone('=') {
                    let level = self.eat_level();
                    results.push(Attribute {name: ident, args: Some(level)});
                    //eprintln!("{} {}", results.last().unwrap().name, results.last().as_ref().unwrap().args.as_ref().unwrap().to_string());
                    assign_form = true;
                    break;
                }
                if !self.open_paren() && !self.open_brace() {
                    results.push(Attribute {name: ident, args: None});
                    break;
                }
                // lets take the whole ts
                results.push(Attribute {name: ident, args: Some(self.eat_level())});
                self.eat_punct_alone(',');
            }
            if !assign_form && !self.eat_eot() {
                break
            }
        }
        results
    }
    
    pub fn eat_all_struct_fields(&mut self,) -> Option<Vec<StructField >> {
        
        if self.open_brace() {
            let mut fields = Vec::new();
            while !self.eat_eot() {
                // lets eat an attrib
                if let Some(sf) = self.eat_struct_field() {
                    fields.push(sf);
                    self.eat_punct_alone(',');
                }
                else {
                    return None
                }
            }
            return Some(fields)
        }
        None
    }
    
    
    pub fn eat_generic(&mut self) -> Option<TokenStream> {
        let mut tb = TokenBuilder::new();
        // if we have a <, keep running and keep a < stack
        
        if self.eat_punct_alone('<') {
            tb.add("<");
            let mut stack = 1;
            // keep eating things till we are at stack 0 for a ">"
            while stack > 0 {
                if self.eat_punct_alone('<') {
                    tb.add("<");
                    stack += 1;
                }
                if self.eat_punct_alone('>') {
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
        None
    }
    
    pub fn eat_all_types(&mut self) -> Option<Vec<TokenStream >> {
        if self.open_paren() {
            let mut ret = Vec::new();
            while !self.eat_eot() {
                self.eat_ident("pub");
                if let Some(tt) = self.eat_type() {
                    ret.push(tt);
                    self.eat_punct_alone(',');
                }
                else {
                    return None
                }
            }
            Some(ret)
        }
        else {
            None
        }
    }
    
    pub fn eat_type(&mut self) -> Option<TokenStream> {
        let mut tb = TokenBuilder::new();
        if self.eat_punct_alone('&'){
            tb.add("&");
            if self.eat_punct_any('\''){
                tb.lifetime_mark();
                if let Some((ty, span)) = self.eat_any_ident_with_span() {
                    tb.ident_with_span(&ty, span);
                }
            }
        }
        if self.open_bracket() { // array type
            tb.add("[");
            while !self.eat_eot() {
                if let Some(current) = &self.current {
                    tb.extend(current.clone());
                }
                self.advance();
            }
            tb.add("]");
            return Some(tb.end())
        }
        else if self.open_paren() { // tuple type
            tb.add("(");
            while !self.eat_eot() {
                tb.stream(self.eat_type());
                self.eat_punct_alone(',');
            }
            tb.add(")");
            return Some(tb.end());
        }
        else if let Some((ty, span)) = self.eat_any_ident_with_span() {
            tb.ident_with_span(&ty, span);
            if ty == "dyn" {
                if let Some((ty, span)) = self.eat_any_ident_with_span() {
                    tb.ident_with_span(&ty, span);
                }
            }
            tb.stream(self.eat_generic());
            return Some(tb.end())
        }
        None
    }
    
}

