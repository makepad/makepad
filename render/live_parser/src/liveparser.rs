
use makepad_live_derive::*;
use crate::token::{Token, TokenWithSpan, TokenId};
use std::iter::Cloned;
use std::slice::Iter;
use crate::id::FileId;
use crate::span::Span;
use crate::liveerror::LiveError;
use crate::liveerror::LiveErrorOrigin;
use crate::id::Id;
use crate::id::IdPack;
use crate::id::IdUnpack;
use crate::livedocument::LiveDocument;
use crate::livenode::{LiveNode, LiveValue};

pub struct LiveParser<'a> {
    pub token_index: usize,
    pub file_id: FileId,
    pub tokens_with_span: Cloned<Iter<'a, TokenWithSpan >>,
    pub token_with_span: TokenWithSpan,
    pub end: usize,
}

impl<'a> LiveParser<'a> {
    pub fn new(tokens: &'a [TokenWithSpan]) -> Self {
        let mut tokens_with_span = tokens.iter().cloned();
        let token_with_span = tokens_with_span.next().unwrap();
        LiveParser {
            file_id: FileId::default(),
            tokens_with_span,
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
    
    fn expect_token(&mut self, expected: Token) -> Result<(), LiveError> {
        let actual = self.peek_token();
        if actual != expected {
            return Err(self.error(format!("expected {} unexpected token `{}`", expected, actual)));
        }
        self.skip_token();
        Ok(())
    }
    
    fn begin_span(&self) -> SpanTracker {
        SpanTracker {
            file_id: self.token_with_span.span.file_id(),
            start: self.token_with_span.span.start(),
        }
    }
    
    fn expect_class_id_wildcard(&mut self, ld: &mut LiveDocument) -> Result<IdPack, LiveError> {
        
        let base = match self.peek_token() {
            token_punct!(*) => {
                self.skip_token();
                Id(0)
            }
            Token::Ident(id) => {
                self.skip_token();
                id
            },
            other => {
                return Err(self.error(format!("Unexpected token after :: {}", other)));
            }
        };
        
        if self.peek_token() == token_punct!(::) {
            self.skip_token();
            // start a multi_id
            let multi_index = ld.multi_ids.len();
            ld.multi_ids.push(base);
            loop {
                match self.peek_token() {
                    token_punct!(*) => {
                        self.skip_token();
                        ld.multi_ids.push(Id::empty());
                        break
                    }
                    Token::Ident(id) => {
                        self.skip_token();
                        ld.multi_ids.push(id);
                        if !self.accept_token(token_punct!(::)) {
                            break;
                        }
                    },
                    other => {
                        return Err(self.error(format!("Unexpected token after :: {}", other)));
                    }
                }
            };
            let id = IdPack::multi(multi_index, ld.multi_ids.len() - multi_index);
            Ok(id)
        }
        else {
            Ok(IdPack::single(base))
        }
    }
    
    fn expect_class_id(&mut self, ld: &mut LiveDocument) -> Result<IdPack, LiveError> {
        
        let base = self.expect_ident() ?;
        
        if self.peek_token() == token_punct!(::) {
            self.skip_token();
            // start a multi_id
            let multi_index = ld.multi_ids.len();
            ld.multi_ids.push(base);
            loop {
                match self.peek_token() {
                    Token::Ident(id) => {
                        self.skip_token();
                        ld.multi_ids.push(id);
                        if !self.accept_token(token_punct!(::)) {
                            break;
                        }
                    },
                    other => {
                        return Err(self.error(format!("Unexpected token after :: {}", other)));
                    }
                }
            };
            Ok(IdPack::multi(multi_index, ld.multi_ids.len() - multi_index))
        }
        else {
            Ok(IdPack::single(base))
        }
    }
    
    fn expect_prop_id(&mut self, ld: &mut LiveDocument) -> Result<IdPack, LiveError> {
        let base = self.expect_ident() ?;
        if self.peek_token() == token_punct!(.) {
            self.skip_token();
            // start a multi_id
            let multi_index = ld.multi_ids.len();
            ld.multi_ids.push(base);
            loop {
                match self.peek_token() {
                    Token::Ident(id) => {
                        self.skip_token();
                        ld.multi_ids.push(id);
                        if !self.accept_token(token_punct!(.)) {
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
    }
    
    fn expect_object(&mut self, level: usize, ld: &mut LiveDocument) -> Result<(u32, u32), LiveError> {
        let node_start = ld.get_level_len(level);
        while self.peek_token() != Token::Eof {
            if self.peek_token() == Token::CloseBrace {
                self.skip_token();
                let node_end = ld.get_level_len(level);
                return Ok((node_start as u32, (node_end - node_start) as u32))
            }
            //let span = self.begin_span();
            self.expect_live_value(IdPack::empty(), level, ld) ?;
            self.expect_token(token_punct!(:)) ?;
            //let span = self.begin_span();
            self.expect_live_value(IdPack::empty(), level, ld) ?;
            if !self.accept_token(token_punct!(,)) {
                self.accept_token(token_punct!(;));
            }
        }
        return Err(self.error(format!("Eof in object body")))
    }
    
    fn expect_array(&mut self, level: usize, ld: &mut LiveDocument) -> Result<(u32, u32), LiveError> {
        let node_start = ld.get_level_len(level);
        while self.peek_token() != Token::Eof {
            if self.peek_token() == Token::CloseBracket {
                self.skip_token();
                let node_end = ld.get_level_len(level);
                return Ok((node_start as u32, (node_end - node_start) as u32))
            }
            //let span = self.begin_span();
            self.expect_live_value(IdPack::empty(), level, ld) ?;
            self.accept_token(token_punct!(,));
        }
        return Err(self.error(format!("Eof in object body")))
    }
    
    fn expect_arguments(&mut self, level: usize, ld: &mut LiveDocument) -> Result<(u32, u16), LiveError> {
        let node_start = ld.get_level_len(level);
        while self.peek_token() != Token::Eof {
            if self.peek_token() == Token::CloseParen {
                self.skip_token();
                let node_end = ld.get_level_len(level);
                return Ok((node_start as u32, (node_end - node_start) as u16))
            }
            //let span = self.begin_span();
            self.expect_live_value(IdPack::empty(), level, ld) ?;
            self.accept_token(token_punct!(,));
        }
        return Err(self.error(format!("Eof in object body")))
    }
    
    fn get_token_id(&self) -> TokenId {
        TokenId {
            file_id: self.file_id,
            token_id: self.token_index as u32
        }
    }
    
    fn expect_live_value(&mut self, prop_id: IdPack, level: usize, ld: &mut LiveDocument) -> Result<(), LiveError> {
        
        // now we can have an array or a class instance
        match self.peek_token() {
            Token::OpenBrace => { // key/value map
                let token_id = self.get_token_id();
                self.skip_token();
                let (node_start, node_count) = self.expect_object(level + 1, ld) ?;
                ld.push_node(level, LiveNode {
                    token_id,
                    id_pack: prop_id,
                    value: LiveValue::Object {node_start, node_count}
                });
            },
            Token::OpenBracket => { // array
                let token_id = self.get_token_id();
                self.skip_token();
                let (node_start, node_count) = self.expect_array(level + 1, ld) ?;
                ld.push_node(level, LiveNode {
                    token_id,
                    id_pack: prop_id,
                    value: LiveValue::Array {node_start, node_count}
                });
            },
            Token::Bool(val) => {
                let token_id = self.get_token_id();
                self.skip_token();
                ld.push_node(level, LiveNode {
                    token_id,
                    id_pack: prop_id,
                    value: LiveValue::Bool(val)
                });
            },
            Token::Int(val) => {
                let token_id = self.get_token_id();
                self.skip_token();
                ld.push_node(level, LiveNode {
                    token_id,
                    id_pack: prop_id,
                    value: LiveValue::Int(val)
                });
            },
            Token::Float(val) => {
                let token_id = self.get_token_id();
                self.skip_token();
                ld.push_node(level, LiveNode {
                    token_id,
                    id_pack: prop_id,
                    value: LiveValue::Float(val)
                });
            },
            Token::Color(val) => {
                let token_id = self.get_token_id();
                self.skip_token();
                ld.push_node(level, LiveNode {
                    token_id,
                    id_pack: prop_id,
                    value: LiveValue::Color(val)
                });
            },
            Token::String {index, len} => {
                let token_id = self.get_token_id();
                self.skip_token();
                ld.push_node(level, LiveNode {
                    token_id,
                    id_pack: prop_id,
                    value: LiveValue::String {string_start: index, string_count: len}
                });
            },
            token_ident!(vec2) => { todo!()
            },
            token_ident!(vec3) => { todo!()
            },
            Token::Ident(_) => { // we're gonna parse a class.
                // we also support vec2/vec3 values directly.
                let token_id = self.get_token_id();
                let target_id = self.expect_class_id(ld) ?;
                if self.accept_token(Token::OpenBrace) {
                    let (node_start, node_count) = self.expect_live_class(level + 1, ld) ?;
                    ld.push_node(level, LiveNode {
                        token_id,
                        id_pack: prop_id,
                        value: LiveValue::Class {class: target_id, node_start, node_count}
                    });
                }
                else if self.accept_token(Token::OpenParen) {
                    let (node_start, node_count) = self.expect_arguments(level + 1, ld) ?;
                    ld.push_node(level, LiveNode {
                        token_id,
                        id_pack: prop_id,
                        value: LiveValue::Call {target: target_id, node_start, node_count}
                    });
                }
                else {
                    ld.push_node(level, LiveNode {
                        token_id,
                        id_pack: prop_id,
                        value: LiveValue::IdPack(target_id)
                    });
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
        match self.peek_token(){
            Token::Bool(_) 
            | Token::Int(_)
            | Token::Float(_)
            | Token::Color(_)=>{
                self.skip_token();
                return Ok(())
            }
            token_ident!(vec2)=>{todo!()}
            token_ident!(vec3)=>{todo!()}
            _=>()
        }
        Err(self.error(format!("Expected value literal")))
    }
    
    fn expect_live_class(&mut self, level: usize, ld: &mut LiveDocument) -> Result<(u32, u16), LiveError> {
        let node_start = ld.get_level_len(level);
        while self.peek_token() != Token::Eof {
            match self.peek_token() {
                Token::CloseBrace => {
                    self.skip_token();
                    let node_end = ld.get_level_len(level);
                    return Ok((node_start as u32, (node_end - node_start) as u16))
                }
                Token::Ident(_) => {
                    //let span = self.begin_span();
                    let token_start = self.token_index;
                    let prop_id = self.expect_prop_id(ld) ?;
                    
                    if let Token::Ident(_) = self.peek_token() {
                        match prop_id {
                            id_pack!(fn) => {
                                //let span = self.begin_span();
                                //self.skip_token();
                                let token_id = self.get_token_id();
                                let prop_id = self.expect_ident() ?;
                                let token_start = self.token_index;
                                let token_index = self.scan_to_token(Token::CloseBrace) ?;
                                
                                ld.push_node(level, LiveNode {
                                    token_id,
                                    id_pack: IdPack::single(prop_id),
                                    value: LiveValue::Fn {
                                        token_start: token_start as u32,
                                        token_count: (token_index - token_start) as u32,
                                        scope_start: 0,
                                        scope_count: 0
                                    }
                                });
                            }
                            id_pack!(use) => {
                                let crate_name = self.expect_ident() ?;
                                self.expect_token(token_punct!(::)) ?;
                                let module_name = self.expect_ident() ?;
                                self.expect_token(token_punct!(::)) ?;
                                // then we have a chain of idents with a possible *
                                let token_id = self.get_token_id();
                                let id = self.expect_class_id_wildcard(ld) ?;
                                
                                let crate_module = ld.create_multi_id(&[crate_name, module_name]);
                                // alright we have an id thats either a * or a chain.
                                ld.push_node(level, LiveNode {
                                    token_id,
                                    id_pack: id,
                                    value: LiveValue::Use {crate_module}
                                });
                                if !self.accept_token(token_punct!(,)) {
                                    self.accept_token(token_punct!(;));
                                }
                            }
                            _ => {
                                // ok so we get an ident.
                                let token_id = self.get_token_id();
                                let ty = self.expect_class_id(ld) ?;
                                if self.accept_token(token_punct!(:)) { // its a vardef
                                    self.expect_var_def_type() ?;
                                    // now an assignment might follow.

                                    // we should parse full expressions here
                                    // consts can only depend on other consts, not on live values
                                    if self.accept_token(token_punct!(=)){
                                        self.expect_value_literal()?;
                                    }
                                    
                                    ld.push_node(level, LiveNode {
                                        token_id,
                                        id_pack: ty,
                                        value: LiveValue::VarDef {
                                            token_start: token_start as u32,
                                            token_count: (self.token_index - token_start) as u32,
                                            scope_start: 0,
                                            scope_count: 0
                                        }
                                    });
                                }
                                else { // its a var ref
                                    ld.push_node(level, LiveNode {
                                        token_id,
                                        id_pack: prop_id,
                                        value: LiveValue::ResourceRef {
                                            target:ty
                                        }
                                    });
                                }
                                if !self.accept_token(token_punct!(,)) {
                                    self.accept_token(token_punct!(;));
                                }
                            }
                        }
                    }
                    else {
                        self.expect_token(token_punct!(:)) ?;
                        // ok now we get a value to parse
                        self.expect_live_value(prop_id, level, ld) ?;
                        if !self.accept_token(token_punct!(,)) {
                            self.accept_token(token_punct!(;));
                        }
                    }
                },
                other => return Err(self.error(format!("Unexpected token {} in class body", other)))
            }
        }
        if level == 0 {
            let node_end = ld.get_level_len(level);
            return Ok((node_start as u32, (node_end - node_start) as u16))
        }
        return Err(self.error(format!("Eof in class body")))
    }
    
    pub fn parse_live_document(&mut self) -> Result<LiveDocument, LiveError> {
        let mut ld = LiveDocument::new();
        self.expect_live_class(0, &mut ld) ?;
        // we should s
        Ok(ld)
    }
}

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
    
    pub fn error(&self, parser: &mut LiveParser, origin:LiveErrorOrigin, message: String) -> LiveError {
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
}
