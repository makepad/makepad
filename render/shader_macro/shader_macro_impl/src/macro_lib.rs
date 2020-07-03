 use proc_macro::{TokenTree, Span, TokenStream, Delimiter, Group, Literal, Ident, Punct, Spacing};


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
    
    pub fn extend(&mut self, tt: TokenTree) -> &mut Self {
        let end = self.groups.len() - 1;
        self.groups[end].1.extend(Some(tt));
        self
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
                ":" | "::" | "," | "!" | "." | "<<" | ">>" | 
                "->" | "=>" | "<" | ">" | "<=" | ">=" | "=" | "==" | "!=" |
                "+" | "+=" | "-" | "-=" | "*" | "*=" | "/" | "/="  => self.punct(part),
                _ => self.ident(part) // this will error if its not an identifier
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
    pub fn _lit(&mut self, lit: Literal) -> &mut Self {self.extend(TokenTree::from(lit))}
    
    pub fn push_group(&mut self, delim: Delimiter) -> &mut Self {
        self.groups.push((delim, TokenStream::new()));
        self
    }
    
    pub fn pop_group(&mut self, delim: Delimiter) -> &mut Self {
        if self.groups.len() < 2 {
            panic!("pop_group stack is empty {}", self.groups.len());
        }
        let ts = self.groups.pop().unwrap();
        if ts.0 != delim {
            panic!("pop_group Delimiter mismatch, got {:?} expected {:?}", ts.0, delim);
        }
        self.extend(TokenTree::from(Group::new(delim,ts.1)));
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

