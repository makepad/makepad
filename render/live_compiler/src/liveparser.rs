
use makepad_id_macros::*;
//use std::collections::{HashMap};
use crate::token::{Token, TokenWithSpan, TokenId};
use std::iter::Cloned;
use std::slice::Iter;
use crate::id::FileId;
use crate::span::Span;
use crate::liveerror::LiveError;
use crate::liveerror::LiveErrorOrigin;
use crate::id::Id;
//use crate::id::LocalPtr;
use crate::livedocument::LiveDocument;
use crate::livenode::{LiveNode, LiveValue, LiveType};
/*
pub struct LiveEnumInfo{
    pub base_name: Id,
    pub bare: Vec<Id>,
    pub named: Vec<Id>,
    pub tuple: Vec<Id>
}*/

pub struct LiveParser<'a> {
    pub token_index: usize,
    pub file_id: FileId,
    pub live_types: &'a [LiveType],
    //pub live_enums: &'a HashMap<LiveType, LiveEnumInfo>,
    pub tokens_with_span: Cloned<Iter<'a, TokenWithSpan >>,
    pub token_with_span: TokenWithSpan,
    pub end: usize,
}

impl<'a> LiveParser<'a> {
    pub fn new(tokens: &'a [TokenWithSpan], live_types: &'a [LiveType], /*live_enums: &'a HashMap<LiveType, LiveEnumInfo>,*/ file_id: FileId) -> Self {
        let mut tokens_with_span = tokens.iter().cloned();
        let token_with_span = tokens_with_span.next().unwrap();
        LiveParser {
            //live_enums,
            file_id,
            tokens_with_span,
            live_types,
            token_with_span,
            token_index: 0,
            end: 0,
        }
    }
}

impl<'a> LiveParser<'a> {
    
    fn peek_span(&self) -> Span {
        self.token_with_span.span
    }
    
    fn peek_token(&self) -> Token {
        self.token_with_span.token
    }
    
    fn eat_token(&mut self) -> Token {
        let token = self.peek_token();
        self.skip_token();
        token
    }
    
    fn skip_token(&mut self) {
        self.end = self.token_with_span.span.end() as usize;
        self.token_with_span = self.tokens_with_span.next().unwrap();
        self.token_index += 1;
    }
    
    fn error(&mut self, message: String) -> LiveError {
        LiveError {
            origin: live_error_origin!(),
            span: self.token_with_span.span,
            message,
        }
    }
    
    
    fn end(&self) -> usize {
        self.end
    }
    
    fn token_end(&self) -> usize {
        self.token_with_span.span.end()
    }
    
    fn accept_ident(&mut self) -> Option<Id> {
        if let Token::Ident(id) = self.peek_token() {
            self.skip_token();
            Some(id)
        }
        else {
            None
        }
    }
    
    fn accept_token(&mut self, token: Token) -> bool {
        if self.peek_token() != token {
            return false;
        }
        self.skip_token();
        true
    }
    
    fn expect_ident(&mut self) -> Result<Id, LiveError> {
        match self.peek_token() {
            Token::Ident(ident) => {
                self.skip_token();
                Ok(ident)
            }
            token => Err(self.error(format!("expected ident, unexpected token `{}`", token))),
        }
    }
    
    fn expect_int(&mut self) -> Result<i64, LiveError> {
        match self.peek_token() {
            Token::Int(v) => {
                self.skip_token();
                Ok(v)
            }
            token => Err(self.error(format!("expected int, unexpected token `{}`", token))),
        }
    }
    
    
    fn expect_token(&mut self, expected: Token) -> Result<(), LiveError> {
        let actual = self.peek_token();
        if actual != expected {
            return Err(self.error(format!("expected {} unexpected token `{}`", expected, actual)));
        }
        self.skip_token();
        Ok(())
    }
    /*
    fn begin_span(&self) -> SpanTracker {
        SpanTracker {
            file_id: self.token_with_span.span.file_id(),
            start: self.token_with_span.span.start(),
        }
    }
    */
    fn expect_use(&mut self, ld: &mut LiveDocument) -> Result<(), LiveError> {
        self.skip_token();
        let token_id = self.get_token_id();
        let crate_id = self.expect_ident() ?;
        self.expect_token(Token::Punct(id!(::))) ?;
        let module_id = self.expect_ident() ?;
        self.expect_token(Token::Punct(id!(::))) ?;
        let object_id = if self.peek_token() == Token::Punct(id!(*)) {
            self.skip_token();
            Id(0)
        }
        else {
            self.expect_ident() ?
        };
        
        ld.nodes.push(LiveNode {
            token_id:Some(token_id),
            id: Id::empty(),
            value: LiveValue::Use {crate_id, module_id, object_id}
        });
        self.accept_optional_delim();
        
        Ok(())
    }
    
    fn expect_fn(&mut self, ld: &mut LiveDocument) -> Result<(), LiveError> {
        let token_start = self.token_index;
        self.skip_token();
        let token_id = self.get_token_id();
        let prop_id = self.expect_ident() ?;
        //let token_start = self.token_index;
        let token_index = self.scan_to_token(Token::CloseBrace) ?;
        
        ld.nodes.push(LiveNode {
            token_id:Some(token_id),
            id: prop_id,
            value: LiveValue::Fn {
                token_start: token_start,
                token_count: (token_index - token_start),
                scope_start: 0,
                scope_count: 0
            }
        });
        
        Ok(())
    }
    
    fn expect_const(&mut self, ld: &mut LiveDocument) -> Result<(), LiveError> {
        let token_start = self.token_index;
        self.skip_token();
        let token_id = self.get_token_id();
        let const_id = self.expect_ident() ?;
        self.expect_token(Token::Punct(id!(:))) ?;
        self.expect_ident() ?;
        self.expect_token(Token::Punct(id!( =))) ?;
        self.expect_value_literal() ?;
        
        ld.nodes.push(LiveNode {
            token_id:Some(token_id),
            id: const_id,
            value: LiveValue::Const {
                token_start: token_start,
                token_count: self.token_index - token_start,
                scope_start: 0,
                scope_count: 0
            }
        });
        
        Ok(())
    }
    
    fn expect_var_def(&mut self, ld: &mut LiveDocument) -> Result<(), LiveError> {
        let token_start = self.token_index;
        let token_id = self.get_token_id();
        let real_prop_id = self.expect_ident() ?;
        self.expect_token(Token::Punct(id!(:))) ?;
        self.expect_var_def_type() ?;
        
        ld.nodes.push(LiveNode {
            token_id:Some(token_id),
            id: real_prop_id,
            value: LiveValue::VarDef {
                token_start,
                token_count: (self.token_index - token_start),
                scope_start: 0,
                scope_count: 0
            }
        });
        
        if self.accept_token(Token::Punct(id!( =))) {
            // ok we now emit a value
            self.expect_live_value(real_prop_id, ld) ?;
        }
        
        Ok(())
    }
    
    /*
    fn expect_prop_id(&mut self, ld: &mut LiveDocument) -> Result<IdPack, LiveError> {
        let base = self.expect_ident() ?;
        if self.peek_token() == Token::Punct(id!(.)) {
            self.skip_token();
            // start a multi_id
            let multi_index = ld.multi_ids.len();
            ld.multi_ids.push(base);
            loop {
                match self.peek_token() {
                    Token::Ident(id) => {
                        self.skip_token();
                        ld.multi_ids.push(id);
                        if !self.accept_token(Token::Punct(id!(.))) {
                            break;
                        }
                    },
                    other => {
                        return Err(self.error(format!("Unexpected token after . {}", other)));
                    }
                }
            };
            Ok(IdPack::multi(multi_index, ld.multi_ids.len() - multi_index))
        }
        else {
            Ok(IdPack::single(base))
        }
    }*/
    /*
    fn expect_object(&mut self, level: usize, ld: &mut LiveDocument) -> Result<(u32, u32), LiveError> {
        let node_start = ld.get_level_len(level);
        while self.peek_token() != Token::Eof {
            if self.peek_token() == Token::CloseBrace {
                self.skip_token();
                let node_end = ld.get_level_len(level);
                return Ok((node_start as u32, (node_end - node_start) as u32))
            }
            //let span = self.begin_span();
            self.expect_live_value(Id::empty(), level, ld) ?;
            self.expect_token(Token::Punct(id!(:))) ?;
            //let span = self.begin_span();
            self.expect_live_value(IdPack::empty(), level, ld) ?;
            if !self.accept_token(Token::Punct(id!(,))) {
                self.accept_token(Token::Punct(id!(;)));
            }
        }
        return Err(self.error(format!("Eof in object body")))
    }*/
    
    fn expect_array(&mut self, prop_id: Id, ld: &mut LiveDocument) -> Result<(), LiveError> {
        self.expect_token(Token::OpenBracket) ?;
        ld.nodes.push(LiveNode {
            token_id: Some(self.get_token_id()),
            id: prop_id,
            value: LiveValue::Array
        });
        while self.peek_token() != Token::Eof {
            if self.accept_token(Token::CloseBracket) {
                ld.nodes.push(LiveNode {
                    token_id: Some(self.get_token_id()),
                    id: Id::empty(),
                    value: LiveValue::Close
                });
                return Ok(())
            }
            self.expect_live_value(Id::empty(), ld) ?;
            self.accept_token(Token::Punct(id!(,)));
        }
        return Err(self.error(format!("Eof in array body")))
    }
    
    fn expect_tuple_enum(&mut self, prop_id: Id, base: Id, variant: Id, ld: &mut LiveDocument) -> Result<(), LiveError> {
        self.expect_token(Token::OpenParen) ?;
        ld.nodes.push(LiveNode {
            token_id: Some(self.get_token_id()),
            id: prop_id,
            value: LiveValue::TupleEnum {base, variant}
        });
        while self.peek_token() != Token::Eof {
            if self.accept_token(Token::CloseParen) {
                ld.nodes.push(LiveNode {
                    token_id: Some(self.get_token_id()),
                    id: Id::empty(),
                    value: LiveValue::Close
                });
                return Ok(())
            }
            //let span = self.begin_span();
            self.expect_live_value(Id::empty(), ld) ?;
            self.accept_token(Token::Punct(id!(,)));
        }
        return Err(self.error(format!("Eof in object body")))
    }
    
    
    fn expect_named_enum(&mut self, prop_id: Id, base: Id, variant: Id, ld: &mut LiveDocument) -> Result<(), LiveError> {
        self.expect_token(Token::OpenBrace) ?;
        
        ld.nodes.push(LiveNode {
            token_id: Some(self.get_token_id()),
            id: prop_id,
            value: LiveValue::NamedEnum {base, variant}
        });
        
        while self.peek_token() != Token::Eof {
            if self.accept_token(Token::CloseBrace) {
                ld.nodes.push(LiveNode {
                    token_id: Some(self.get_token_id()),
                    id: Id::empty(),
                    value: LiveValue::Close
                });
                return Ok(())
            }
            let prop_id = self.expect_ident() ?;
            
            self.expect_token(Token::Punct(id!(:))) ?;
            self.expect_live_value(prop_id, ld) ?;
            self.accept_token(Token::Punct(id!(,)));
        }
        return Err(self.error(format!("Eof in named enum")))
    }
    
    fn get_token_id(&self) -> TokenId {
        TokenId::new(self.file_id, self.token_index)
    }
    
    fn expect_live_value(&mut self, prop_id: Id, ld: &mut LiveDocument) -> Result<(), LiveError> {
       
        // now we can have an array or a class instance
        match self.peek_token() {
            Token::OpenBrace => { // key/value map
                self.skip_token();
                let token_id = self.get_token_id();
                // if we get an OpenBrace immediately after, we are a rust_type
                if self.peek_token() == Token::OpenBrace {
                    let val = self.expect_int() ?;
                    
                    if val< 0 || val >= self.live_types.len() as i64 {
                        return Err(self.error(format!("live_type index out of range {}", val)));
                    }
                    ld.nodes.push(LiveNode {
                        token_id:Some(token_id),
                        id: prop_id,
                        value: LiveValue::LiveType(self.live_types[val as usize])
                    });
                    self.expect_token(Token::CloseBrace) ?;
                    self.expect_token(Token::CloseBrace) ?;
                    return Ok(());
                }
                else {
                    ld.nodes.push(LiveNode {
                        token_id:Some(token_id),
                        id: prop_id,
                        value: LiveValue::BareClass
                    });
                    self.expect_live_class(false, prop_id, ld) ?;
                }
            },
            Token::OpenBracket => { // array
                self.expect_array(prop_id, ld) ?;
            },
            Token::Bool(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    token_id: Some(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::Bool(val)
                });
            },
            Token::Int(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    token_id: Some(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::Int(val)
                });
            },
            Token::Float(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    token_id: Some(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::Float(val)
                });
            },
            Token::Color(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    token_id: Some(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::Color(val)
                });
            },
            Token::String {index, len} => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    token_id: Some(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::StringRef {string_start: index as usize, string_count: len as usize}
                });
            },
            Token::Ident(id!(vec2)) => {todo!()},
            Token::Ident(id!(vec3)) => {todo!()},
            Token::Ident(base) => { // we're gonna parse a class or an enum
                self.skip_token();
                if self.accept_token(Token::Punct(id!(::))) { // enum
                    let variant = self.expect_ident() ?;
                    match self.peek_token() {
                        Token::OpenBrace => {
                            self.expect_named_enum(prop_id, base, variant, ld) ?;
                        }
                        Token::OpenParen => {
                            self.expect_tuple_enum(prop_id, base, variant, ld) ?;
                        }
                        _ => {
                            ld.nodes.push(LiveNode {
                                token_id: Some(self.get_token_id()),
                                id: prop_id,
                                value: LiveValue::BareEnum {base, variant}
                            })
                        }
                    }
                }
                else { // its an ident o
                    ld.nodes.push(LiveNode {
                        token_id: Some(self.get_token_id()),
                        id: prop_id,
                        value: LiveValue::NamedClass {class: base}
                    });
                    self.expect_token(Token::OpenBrace)?;
                    self.expect_live_class(false, prop_id, ld) ?;
                    
                }
            },
            other => return Err(self.error(format!("Unexpected token {} in property value", other)))
        }
        Ok(())
    }
    
    fn scan_to_token(&mut self, scan_token: Token) -> Result<usize, LiveError> {
        // ok we are going to scan to token, keeping in mind our levels.
        let mut stack_depth = 0;
        
        while self.peek_token() != Token::Eof {
            match self.peek_token() {
                Token::OpenBrace | Token::OpenParen | Token::OpenBracket => {
                    stack_depth += 1;
                }
                Token::CloseBrace | Token::CloseParen | Token::CloseBracket => {
                    if stack_depth == 0 {
                        return Err(self.error(format!("Found closing )}}] whilst scanning for {}", scan_token)));
                    }
                    stack_depth -= 1;
                }
                _ => ()
            }
            if stack_depth == 0 && self.peek_token() == scan_token {
                self.skip_token();
                return Ok(self.token_index)
            }
            self.skip_token();
        }
        return Err(self.error(format!("Could not find ending token {} whilst scanning", scan_token)));
    }
    
    fn expect_var_def_type(&mut self) -> Result<(), LiveError> {
        self.expect_ident() ?;
        if self.accept_token(Token::Ident(id!(in))) {
            self.expect_ident() ?;
        }
        Ok(())
    }
    
    fn expect_value_literal(&mut self) -> Result<(), LiveError> {
        match self.peek_token() {
            Token::Bool(_)
                | Token::Int(_)
                | Token::Float(_)
                | Token::Color(_) => {
                self.skip_token();
                return Ok(())
            }
            Token::Ident(id!(vec2)) => {todo!()}
            Token::Ident(id!(vec3)) => {todo!()}
            _ => ()
        }
        Err(self.error(format!("Expected value literal")))
    }
    
    fn expect_live_class(&mut self, root:bool, prop_id: Id, ld: &mut LiveDocument) -> Result<(), LiveError> {

        while self.peek_token() != Token::Eof {
            match self.peek_token() {
                Token::CloseBrace => {
                    if root{
                        return Err(self.error(format!("Unexpected token }} in root")))
                    }
                    self.skip_token();
                    ld.nodes.push(LiveNode {
                        token_id: Some(self.get_token_id()),
                        id: prop_id,
                        value: LiveValue::Close
                    });
                    return Ok(());
                }
                Token::Ident(prop_id) => {
                    self.skip_token();
                    
                    //let span = self.begin_span();
                    // next 
                    if let Token::Ident(_) = self.peek_token() {
                        match prop_id {
                            id!(fn) => {
                                self.expect_fn(ld) ?;
                                self.accept_optional_delim();
                            }
                            id!(use) => {
                                self.expect_use(ld) ?;
                                self.accept_optional_delim();
                            }
                            id!(const) => {
                                self.expect_const(ld) ?;
                                self.accept_optional_delim();
                            }
                            _ => {
                                // ok so we get an ident.
                                self.expect_var_def(ld) ?;
                                self.accept_optional_delim();
                            }
                        }
                    }
                    else { // has to be key:value
                        self.expect_token(Token::Punct(id!(:))) ?;
                        self.expect_live_value(prop_id, ld) ?;
                        self.accept_optional_delim();
                    }
                },
                other => return Err(self.error(format!("Unexpected token {} in class body of {}", other, prop_id)))
            }
        }
        if root{
            return Ok(())
        }
        return Err(self.error(format!("Eof in class body")))
    }
    
    pub fn accept_optional_delim(&mut self) {
        if !self.accept_token(Token::Punct(id!(,))) {
            self.accept_token(Token::Punct(id!(;)));
        }
    }
    
    pub fn parse_live_document(&mut self) -> Result<LiveDocument, LiveError> {
        let mut ld = LiveDocument::new();
        ld.nodes.push(LiveNode {
            token_id: Some(self.get_token_id()),
            id: Id::empty(),
            value: LiveValue::BareClass
        });
        self.expect_live_class(true, Id::empty(), &mut ld) ?;
        ld.nodes.push(LiveNode {
            token_id: Some(self.get_token_id()),
            id: Id::empty(),
            value: LiveValue::Close
        });
        // we should s
        Ok(ld)
    }
}
/*
pub struct SpanTracker {
    pub file_id: FileId,
    pub start: usize,
}

impl SpanTracker {
    pub fn end(&self, parser: &mut LiveParser) -> Span {
        Span::new(
            self.file_id,
            self.start,
            parser.end()
        )
    }
    
    pub fn error(&self, parser: &mut LiveParser, origin: LiveErrorOrigin, message: String) -> LiveError {
        LiveError {
            origin,
            span: Span::new(
                self.file_id,
                self.start,
                parser.token_end(),
            ),
            message,
        }
    }
}*/
