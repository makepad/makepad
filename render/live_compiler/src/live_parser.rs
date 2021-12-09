use {
    std::{
        iter::Cloned,
        slice::Iter
    },
    makepad_id_macros::*,
    makepad_math::{
        Vec2, Vec3, Vec4
    },
    crate::{
        token::{Token, TokenWithSpan, TokenId},
        live_id::{LiveId, LiveFileId, LiveModuleId},
        span::Span,
        live_error::{LiveError, LiveErrorOrigin},
        live_document::LiveDocument,
        live_node::{LiveNode, LiveValue, LiveTypeInfo, LiveBinOp, LiveUnOp, LiveNodeOrigin, LiveEditInfo},
    }
};

pub struct LiveParser<'a> {
    pub token_index: usize,
    pub file_id: LiveFileId,
    pub live_type_infos: &'a [LiveTypeInfo],
    pub tokens_with_span: Cloned<Iter<'a, TokenWithSpan >>,
    pub token_with_span: TokenWithSpan,
    pub end: usize,
}

impl<'a> LiveParser<'a> {
    pub fn new(tokens: &'a [TokenWithSpan], live_type_infos: &'a [LiveTypeInfo], file_id: LiveFileId) -> Self {
        let mut tokens_with_span = tokens.iter().cloned();
        let token_with_span = tokens_with_span.next().unwrap();
        LiveParser {
            file_id,
            tokens_with_span,
            live_type_infos,
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
    
    fn accept_ident(&mut self) -> Option<LiveId> {
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
    
    fn expect_ident(&mut self) -> Result<LiveId, LiveError> {
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
    
    fn expect_float(&mut self) -> Result<f64, LiveError> {
        match self.peek_token() {
            Token::Float(v) => {
                self.skip_token();
                Ok(v)
            }
            Token::Int(v) => {
                self.skip_token();
                Ok(v as f64)
            }
            token => Err(self.error(format!("expected float, unexpected token `{}`", token))),
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
    
    fn expect_use(&mut self, ld: &mut LiveDocument) -> Result<(), LiveError> {
        let token_id = self.get_token_id();
        let crate_id = self.expect_ident() ?;
        self.expect_token(Token::Punct(id!(::))) ?;
        
        // ok so. we need to collect everything 'upto' the last id
        let first_module_id = self.expect_ident() ?;
        self.expect_token(Token::Punct(id!(::))) ?;
        let mut module = String::new();
        let mut last_id = LiveId(0);
        module.push_str(&format!("{}", first_module_id));
        loop{
            match self.peek_token(){
                Token::Ident(id)=>{
                    self.skip_token();
                    last_id = id;
                    if !self.accept_token(Token::Punct(id!(::))){
                        break;
                    }
                    module.push_str(&format!("::{}", id));
                },
                Token::Punct(id!(*))=>{
                    self.skip_token();
                    last_id = LiveId(0);
                    break;
                }
                _=>{
                    break;
                }
            }
        }

        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(token_id),
            id: last_id,
            value: LiveValue::Use(LiveModuleId(crate_id, LiveId::from_str(&module).unwrap()))
        });
        
        Ok(())
    }
    
    fn expect_fn(&mut self, ld: &mut LiveDocument) -> Result<(), LiveError> {
        let token_start = self.token_index - 1;
        let token_id = self.get_token_id();
        let prop_id = self.expect_ident() ?;
        //let token_start = self.token_index;
        let token_index = self.scan_to_token(Token::CloseBrace) ?;
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(token_id),
            id: prop_id,
            value: LiveValue::DSL {
                token_start: token_start as u32,
                token_count: (token_index - token_start) as u32,
            }
        });
        
        Ok(())
    }
    
    fn expect_const(&mut self, ld: &mut LiveDocument) -> Result<(), LiveError> {
        let token_start = self.token_index - 1;
        let token_id = self.get_token_id();
        let const_id = self.expect_ident() ?;
        self.expect_token(Token::Punct(id!(:))) ?;
        self.expect_ident() ?;
        self.expect_token(Token::Punct(id!( =))) ?;
        self.expect_value_literal() ?;
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(token_id),
            id: const_id,
            value: LiveValue::DSL {
                token_start: token_start as u32,
                token_count: (self.token_index - token_start) as u32,
            }
        });
        
        Ok(())
    }
    
    fn possible_edit_info(&mut self, ld: &mut LiveDocument)->Result<Option<LiveEditInfo>, LiveError>{
        // metadata is .{key:value}
        // lets align to the next index
        let fill_index =  ld.edit_info.len()&0xf;
        for _ in 0..(16-fill_index){
            ld.edit_info.push(LiveNode::empty())
        }
        let edit_info_index = ld.edit_info.len();
        
        ld.edit_info.push(LiveNode{
            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
            id:LiveId::empty(),
            value:LiveValue::Object
        });
        
        if self.accept_token(Token::Punct(id!(.))){
            self.expect_token(Token::OpenBrace)?;
            
            while self.peek_token() != Token::Eof {
                match self.peek_token() {
                    Token::CloseBrace => {
                        self.skip_token();
                        ld.edit_info.push(LiveNode {
                            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                            id: LiveId::empty(),
                            value: LiveValue::Close
                        });
                        return Ok(Some(LiveEditInfo::new(self.file_id, edit_info_index)));
                    }
                    Token::Ident(prop_id) => {
                        self.skip_token();
                        self.expect_token(Token::Punct(id!(:))) ?;

                        match self.peek_token(){
                            Token::Bool(val) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::Bool(val)
                                });
                            },
                            Token::Int(val) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::Int(val)
                                });
                            },
                            Token::Float(val) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::Float(val)
                                });
                            },
                            Token::Color(val) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::Color(val)
                                });
                            },
                            Token::String {index, len} => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::DocumentString {string_start: index as usize, string_count: len as usize}
                                });
                            },
                            other => return Err(self.error(format!("Unexpected token {} in edit_info", other)))
                        }
                        self.accept_optional_delim();
                    },
                    other => return Err(self.error(format!("Unexpected token {} in edit_info", other)))
                }
            }
            return Err(self.error(format!("Eof in edit info")))
        }
        
        Ok(None)
    }
    
    fn expect_var_def(&mut self, ld: &mut LiveDocument) -> Result<(), LiveError> {
        
        let token_start = self.token_index - 1;
        let token_id = self.get_token_id();
        let real_prop_id = self.expect_ident() ?;
        let edit_info = self.possible_edit_info(ld)?;
        self.expect_token(Token::Punct(id!(:))) ?;
        self.expect_var_def_type() ?;
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(token_id),
            id: real_prop_id,
            value: LiveValue::DSL {
                token_start: token_start as u32,
                token_count: (self.token_index - token_start) as u32,
            }
        });
        
        if self.accept_token(Token::Punct(id!( =))) {
            // ok we now emit a value
            self.expect_live_value(real_prop_id, edit_info, ld) ?;
        }
        
        Ok(())
    }
    
    fn expect_array(&mut self, prop_id: LiveId, ld: &mut LiveDocument) -> Result<(), LiveError> {
        self.expect_token(Token::OpenBracket) ?;
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
            id: prop_id,
            value: LiveValue::Array
        });
        while self.peek_token() != Token::Eof {
            if self.accept_token(Token::CloseBracket) {
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                    id: LiveId::empty(),
                    value: LiveValue::Close
                });
                return Ok(())
            }
            self.expect_live_value(LiveId::empty(), None, ld) ?;
            self.accept_token(Token::Punct(id!(,)));
        }
        return Err(self.error(format!("Eof in array body")))
    }
    
    
    fn expect_tuple_enum(&mut self, prop_id: LiveId, base: LiveId, variant: LiveId, ld: &mut LiveDocument) -> Result<(), LiveError> {
        self.expect_token(Token::OpenParen) ?;
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
            id: prop_id,
            value: LiveValue::TupleEnum {base, variant}
        });
        while self.peek_token() != Token::Eof {
            if self.accept_token(Token::CloseParen) {
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::Close
                });
                return Ok(())
            }
            //let span = self.begin_span();
            self.expect_live_value(LiveId::empty(), None, ld) ?;
            self.accept_token(Token::Punct(id!(,)));
        }
        return Err(self.error(format!("Eof in object body")))
    }
    
    
    fn expect_named_enum(&mut self, prop_id: LiveId, base: LiveId, variant: LiveId, ld: &mut LiveDocument) -> Result<(), LiveError> {
        self.expect_token(Token::OpenBrace) ?;
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
            id: prop_id,
            value: LiveValue::NamedEnum {base, variant}
        });
        
        while self.peek_token() != Token::Eof {
            if self.accept_token(Token::CloseBrace) {
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::Close
                });
                return Ok(())
            }
            let prop_id = self.expect_ident() ?;
            let edit_info = self.possible_edit_info(ld)?;
            self.expect_token(Token::Punct(id!(:))) ?;
            self.expect_live_value(prop_id, edit_info, ld) ?;
            self.accept_token(Token::Punct(id!(,)));
        }
        return Err(self.error(format!("Eof in named enum")))
    }
    
    fn get_token_id(&self) -> TokenId {
        TokenId::new(self.file_id, self.token_index)
    }
    
    fn expect_live_value(&mut self, prop_id: LiveId, edit_info:Option<LiveEditInfo>, ld: &mut LiveDocument) -> Result<(), LiveError> {
        // now we can have an array or a class instance
        match self.peek_token() {
            Token::OpenBrace => { // key/value map
                self.skip_token();
                let token_id = self.get_token_id();
                // if we get an OpenBrace immediately after, we are a rust_type
                if self.peek_token() == Token::OpenBrace {
                    self.skip_token();
                    let val = self.expect_int() ?;
                    
                    if val< 0 || val >= self.live_type_infos.len() as i64 {
                        return Err(self.error(format!("live_type index out of range {}", val)));
                    }
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_edit_info(edit_info),
                        id: prop_id,
                        value: LiveValue::Class {
                            live_type: self.live_type_infos[val as usize].live_type,
                            class_parent: None,
                            
                        }
                    });
                    self.expect_token(Token::CloseBrace) ?;
                    self.expect_token(Token::CloseBrace) ?;
                    
                    self.expect_token(Token::OpenBrace) ?;
                    self.expect_live_class(false, prop_id, ld) ?;
                    
                    return Ok(());
                }
                else {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_edit_info(edit_info),
                        id: prop_id,
                        value: LiveValue::Object
                    });
                    self.expect_live_class(false, prop_id, ld) ?;
                }
            },
            Token::OpenParen => { // expression
                self.expect_expression(prop_id, ld) ?;
            },
            Token::OpenBracket => { // array
                self.expect_array(prop_id, ld) ?;
            },
            Token::Bool(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                    id: prop_id,
                    value: LiveValue::Bool(val)
                });
            },
            Token::Int(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                    id: prop_id,
                    value: LiveValue::Int(val)
                });
            },
            Token::Float(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                    id: prop_id,
                    value: LiveValue::Float(val)
                });
            },
            Token::Color(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                    id: prop_id,
                    value: LiveValue::Color(val)
                });
            },
            Token::String {index, len} => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                    id: prop_id,
                    value: LiveValue::DocumentString {string_start: index as usize, string_count: len as usize}
                });
            },
            Token::Ident(id!(vec2)) => {
                self.skip_token();
                self.expect_token(Token::OpenParen) ?;
                let x = self.expect_float() ?;
                self.expect_token(Token::Punct(id!(,))) ?;
                let y = self.expect_float() ?;
                self.expect_token(Token::CloseParen) ?;
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                    id: prop_id,
                    value: LiveValue::Vec2(Vec2 {x: x as f32, y: y as f32})
                });
            },
            Token::Ident(id!(vec3)) => {
                self.skip_token();
                self.expect_token(Token::OpenParen) ?;
                let x = self.expect_float() ?;
                self.expect_token(Token::Punct(id!(,))) ?;
                let y = self.expect_float() ?;
                self.expect_token(Token::Punct(id!(,))) ?;
                let z = self.expect_float() ?;
                self.expect_token(Token::CloseParen) ?;
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                    id: prop_id,
                    value: LiveValue::Vec3(Vec3 {x: x as f32, y: y as f32, z: z as f32})
                });
            },
            Token::Ident(id!(vec4)) => {
                self.skip_token();
                self.expect_token(Token::OpenParen) ?;
                let x = self.expect_float() ?;
                self.expect_token(Token::Punct(id!(,))) ?;
                let y = self.expect_float() ?;
                self.expect_token(Token::Punct(id!(,))) ?;
                let z = self.expect_float() ?;
                self.expect_token(Token::Punct(id!(,))) ?;
                let w = self.expect_float() ?;
                self.expect_token(Token::CloseParen) ?;
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                    id: prop_id,
                    value: LiveValue::Vec4(Vec4 {x: x as f32, y: y as f32, z: z as f32, w: w as f32})
                });
            },
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
                                origin: LiveNodeOrigin::from_token_id(self.get_token_id()).with_edit_info(edit_info),
                                id: prop_id,
                                value: LiveValue::BareEnum {base, variant}
                            })
                        }
                    }
                }
                else { // its an ident o
                    // what if id is followed by
                    // anything but a {/
                    
                    let token_id = self.get_token_id();
                    if self.accept_token(Token::OpenBrace) {
                        ld.nodes.push(LiveNode {
                            origin: LiveNodeOrigin::from_token_id(token_id).with_edit_info(edit_info),
                            id: prop_id,
                            value: LiveValue::Clone(base)
                        });
                        self.expect_live_class(false, prop_id, ld) ?;
                    }
                    else {
                        ld.nodes.push(LiveNode {
                            origin: LiveNodeOrigin::from_token_id(token_id).with_edit_info(edit_info),
                            id: prop_id,
                            value: LiveValue::Id(base)
                        });
                    }
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
    
    fn expect_live_class(&mut self, root: bool, prop_id: LiveId, ld: &mut LiveDocument) -> Result<(), LiveError> {
        
        while self.peek_token() != Token::Eof {
            match self.peek_token() {
                Token::CloseBrace => {
                    if root {
                        return Err(self.error(format!("Unexpected token }} in root")))
                    }
                    self.skip_token();
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                        id: prop_id,
                        value: LiveValue::Close
                    });
                    return Ok(());
                }
                Token::Ident(prop_id) => {
                    self.skip_token();
                    
                    //let span = self.begin_span();
                    // next
                    // there is another token coming
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
                                self.expect_var_def(ld) ?;
                                self.accept_optional_delim();
                            }
                        }
                    }
                    else { // has to be key:value
                        // if we get a . metadata follows
                        let edit_info = self.possible_edit_info(ld)?;
                        self.expect_token(Token::Punct(id!(:))) ?;
                        self.expect_live_value(prop_id, edit_info, ld) ?;
                        self.accept_optional_delim();
                    }
                },
                other => return Err(self.error(format!("Unexpected token {} in class body of {}", other, prop_id)))
            }
        }
        if root {
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
            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
            id: LiveId::empty(),
            value: LiveValue::Object
        });
        self.expect_live_class(true, LiveId::empty(), &mut ld) ?;
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
            id: LiveId::empty(),
            value: LiveValue::Close
        });
        // we should s
        Ok(ld)
    }
    
    
    
    
    
    // EXPRESSION PARSER
    
    
    
    
    fn expect_expression(&mut self, prop_id: LiveId, ld: &mut LiveDocument) -> Result<(), LiveError> {
        
        let expr = self.expect_prim_expr() ?;
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
            id: prop_id,
            value: LiveValue::Expr
        });
        
        fn recur_walk(expr: Expr, ld: &mut LiveDocument) {
            match expr {
                Expr::Bin {token_id, op, left_expr, right_expr} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::ExprBinOp(op)
                    });
                    recur_walk(*left_expr, ld);
                    recur_walk(*right_expr, ld);
                }
                Expr::Un {token_id, op, expr} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::ExprUnOp(op)
                    });
                    recur_walk(*expr, ld);
                }
                Expr::Call {token_id, ident, arg_exprs} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::ExprCall {ident, args: arg_exprs.len()}
                    });
                    for arg in arg_exprs {
                        recur_walk(arg, ld);
                    }
                }
                Expr::Member {token_id, ident, expr} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::ExprMember(ident)
                    });
                    recur_walk(*expr, ld);
                }
                Expr::Var {token_id, ident} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::Id(ident)
                    });
                }
                Expr::Bool {token_id, v} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::Bool(v)
                    });
                }
                Expr::Int {token_id, v} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::Int(v)
                    });
                }
                Expr::Float {token_id, v} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::Float(v)
                    });
                }
                Expr::Color {token_id, v} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: LiveId::empty(),
                        value: LiveValue::Color(v)
                    });
                }
            }
        }
        
        recur_walk(expr, ld);
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
            id: prop_id,
            value: LiveValue::Close
        });
        
        
        Ok(())
        /*
        ld.nodes.push(LiveNode {
            token_id: Some(self.get_token_id()),
            id: prop_id,
            value: LiveValue::Expression
        });
        
        let mut stack_depth = 0;
        
        while self.peek_token() != Token::Eof {
            if self.accept_token(Token::OpenParen){
                stack_depth += 1;             
            }
            else if self.accept_token(Token::CloseParen) {
                stack_depth -= 1;
                // terminate
                if stack_depth == 0{
                    ld.nodes.push(LiveNode {
                        token_id: Some(self.get_token_id()),
                        id: LiveId::empty(),
                        value: LiveValue::Close
                    });
                    return Ok(())
                }
            }
            // ok so what do we do here.
            // we accept 
            self.expect_live_value(LiveId::empty(), ld) ?;
            self.accept_token(Token::Punct(id!(,)));
        }
        return Err(self.error(format!("Eof in array body")))*/
    }
    
    fn expect_expr(&mut self) -> Result<Expr, LiveError> {
        self.expect_cond_expr()
    }
    
    fn expect_cond_expr(&mut self) -> Result<Expr, LiveError> {
        let expr = self.expect_or_expr() ?;
        Ok(if self.accept_token(Token::Punct(id!( ?))) {
            let token_id = self.get_token_id();
            let expr_if_true = self.expect_expr() ?;
            self.expect_token(Token::Punct(id!(:))) ?;
            let expr_if_false = self.expect_cond_expr() ?;
            Expr ::Call {token_id, ident: id!(cond), arg_exprs: vec![expr, expr_if_true, expr_if_false]}
        } else {
            expr
        })
    }
    
    fn expect_or_expr(&mut self) -> Result<Expr, LiveError> {
        let mut acc = self.expect_and_expr() ?;
        while let Some(op) = LiveBinOp::from_or_op(self.peek_token()) {
            self.skip_token();
            let token_id = self.get_token_id();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_and_expr() ?);
            acc = Expr::Bin {token_id, op, left_expr, right_expr,};
        }
        Ok(acc)
    }
    
    fn expect_and_expr(&mut self) -> Result<Expr, LiveError> {
        let mut acc = self.expect_eq_expr() ?;
        while let Some(op) = LiveBinOp::from_and_op(self.peek_token()) {
            self.skip_token();
            let token_id = self.get_token_id();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_eq_expr() ?);
            acc = Expr::Bin {token_id, op, left_expr, right_expr,};
        }
        Ok(acc)
    }
    
    fn expect_eq_expr(&mut self) -> Result<Expr, LiveError> {
        let mut acc = self.expect_rel_expr() ?;
        while let Some(op) = LiveBinOp::from_eq_op(self.peek_token()) {
            self.skip_token();
            let token_id = self.get_token_id();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_rel_expr() ?);
            acc = Expr::Bin {token_id, op, left_expr, right_expr,};
        }
        Ok(acc)
    }
    
    fn expect_rel_expr(&mut self) -> Result<Expr, LiveError> {
        let mut acc = self.expect_add_expr() ?;
        while let Some(op) = LiveBinOp::from_rel_op(self.peek_token()) {
            self.skip_token();
            let token_id = self.get_token_id();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_add_expr() ?);
            acc = Expr::Bin {token_id, op, left_expr, right_expr,};
        }
        Ok(acc)
    }
    
    fn expect_add_expr(&mut self) -> Result<Expr, LiveError> {
        let mut acc = self.expect_mul_expr() ?;
        while let Some(op) = LiveBinOp::from_add_op(self.peek_token()) {
            self.skip_token();
            let token_id = self.get_token_id();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_mul_expr() ?);
            acc = Expr::Bin {token_id, op, left_expr, right_expr,};
        }
        Ok(acc)
    }
    
    fn expect_mul_expr(&mut self) -> Result<Expr, LiveError> {
        let mut acc = self.expect_un_expr() ?;
        while let Some(op) = LiveBinOp::from_mul_op(self.peek_token()) {
            self.skip_token();
            let token_id = self.get_token_id();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_un_expr() ?);
            acc = Expr::Bin {token_id, op, left_expr, right_expr,};
        }
        Ok(acc)
    }
    
    fn expect_un_expr(&mut self) -> Result<Expr, LiveError> {
        Ok(if let Some(op) = LiveUnOp::from_un_op(self.peek_token()) {
            self.skip_token();
            let token_id = self.get_token_id();
            let expr = Box::new(self.expect_un_expr() ?);
            Expr::Un {token_id, op, expr,}
        } else {
            self.expect_member_expr() ?
        })
    }
    
    fn expect_member_expr(&mut self) -> Result<Expr, LiveError> {
        let mut acc = self.expect_prim_expr() ?;
        loop {
            if let Token::Punct(id!(.)) = self.peek_token() {
                self.skip_token();
                let token_id = self.get_token_id();
                let ident = self.expect_ident() ?;
                acc = Expr::Member {token_id, ident, expr: Box::new(acc)}
            }
            else {
                break
            }
        }
        Ok(acc)
    }
    
    fn expect_prim_expr(&mut self) -> Result<Expr, LiveError> {
        match self.peek_token() {
            Token::Ident(ident) => {
                let token_id = self.get_token_id();
                self.skip_token();
                match self.peek_token() {
                    Token::OpenParen => {
                        let arg_exprs = self.expect_arg_exprs() ?;
                        Ok(Expr::Call {token_id, ident, arg_exprs})
                    }
                    _ => Ok(Expr::Var {token_id, ident}),
                }
            }
            Token::Bool(v) => {
                self.skip_token();
                let token_id = self.get_token_id();
                Ok(Expr::Bool {token_id, v})
            }
            Token::Int(v) => {
                self.skip_token();
                let token_id = self.get_token_id();
                Ok(Expr::Int {token_id, v})
            }
            Token::Float(v) => {
                self.skip_token();
                let token_id = self.get_token_id();
                Ok(Expr::Float {token_id, v})
            }
            Token::Color(v) => {
                self.skip_token();
                let token_id = self.get_token_id();
                Ok(Expr::Color {token_id, v})
            }
            Token::OpenParen => {
                self.skip_token();
                let expr = self.expect_expr() ?;
                self.expect_token(Token::CloseParen) ?;
                Ok(expr)
            }
            token => Err(self.error(format!("Unexpected token {} in class expression", token)))
        }
    }
    
    fn expect_arg_exprs(&mut self) -> Result<Vec<Expr>, LiveError> {
        self.expect_token(Token::OpenParen) ?;
        let mut arg_exprs = Vec::new();
        if !self.accept_token(Token::CloseParen) {
            loop {
                arg_exprs.push(self.expect_expr() ?);
                if !self.accept_token(Token::Punct(id!(,))) {
                    break;
                }
            }
            self.expect_token(Token::CloseParen) ?;
        }
        Ok(arg_exprs)
    }
    
    
}

#[derive(Debug)]
enum Expr {
    Bin {
        token_id: TokenId,
        op: LiveBinOp,
        left_expr: Box<Expr>,
        right_expr: Box<Expr>,
    },
    Un {
        token_id: TokenId,
        op: LiveUnOp,
        expr: Box<Expr>,
    },
    Call {
        token_id: TokenId,
        ident: LiveId,
        arg_exprs: Vec<Expr>,
    },
    Member {
        token_id: TokenId,
        ident: LiveId,
        expr: Box<Expr>
    },
    Var {
        token_id: TokenId,
        ident: LiveId,
    },
    Bool {
        token_id: TokenId,
        v: bool
    },
    Int {
        token_id: TokenId,
        v: i64
    },
    Float {
        token_id: TokenId,
        v: f64
    },
    Color {
        token_id: TokenId,
        v: u32
    }
}


impl LiveBinOp {
    fn from_or_op(token: Token) -> Option<Self> {
        match token {
            Token::Punct(id!( ||)) => Some(Self::Or),
            _ => None,
        }
    }
    
    fn from_and_op(token: Token) -> Option<Self> {
        match token {
            Token::Punct(id!( &&)) => Some(Self::And),
            _ => None,
        }
    }
    
    fn from_eq_op(token: Token) -> Option<Self> {
        match token {
            Token::Punct(id!( ==)) => Some(Self::Eq),
            Token::Punct(id!( !=)) => Some(Self::Ne),
            _ => None,
        }
    }
    
    fn from_rel_op(token: Token) -> Option<Self> {
        match token {
            Token::Punct(id!(<)) => Some(Self::Lt),
            Token::Punct(id!( <=)) => Some(Self::Le),
            Token::Punct(id!(>)) => Some(Self::Gt),
            Token::Punct(id!( >=)) => Some(Self::Ge),
            _ => None,
        }
    }
    
    fn from_add_op(token: Token) -> Option<Self> {
        match token {
            Token::Punct(id!( +)) => Some(Self::Add),
            Token::Punct(id!(-)) => Some(Self::Sub),
            _ => None,
        }
    }
    
    fn from_mul_op(token: Token) -> Option<Self> {
        match token {
            Token::Punct(id!(*)) => Some(Self::Mul),
            Token::Punct(id!( /)) => Some(Self::Div),
            _ => None,
        }
    }
}

impl LiveUnOp {
    pub fn from_un_op(token: Token) -> Option<Self> {
        match token {
            Token::Punct(id!(!)) => Some(Self::Not),
            Token::Punct(id!(-)) => Some(Self::Neg),
            _ => None,
        }
    }
}

