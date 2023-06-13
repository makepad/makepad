use {
    std::{
        rc::Rc,
        iter::Cloned,
        slice::Iter
    },
    crate::{
        makepad_live_tokenizer::{LiveId, Delim},
        makepad_live_tokenizer::{LiveErrorOrigin, live_error_origin},
        makepad_live_id::*,
        makepad_math::{
            Vec2,
            Vec3,
            Vec4
        },
        live_token::{LiveToken, TokenWithSpan, LiveTokenId},
        live_ptr::{LiveFileId, LiveModuleId},
        span::{TextSpan, TextPos},
        live_error::{LiveError},
        live_document::LiveOriginal,
        live_node::{LivePropType, LiveNode, LiveValue, LiveTypeInfo, LiveBinOp, LiveUnOp, LiveNodeOrigin, LiveEditInfo},
    }
};

pub struct LiveParser<'a> {
    pub token_index: usize,
    pub file_id: LiveFileId,
    pub live_type_info_counter: usize,
    pub live_type_infos: &'a [LiveTypeInfo],
    pub tokens_with_span: Cloned<Iter<'a, TokenWithSpan >>,
    pub token_with_span: TokenWithSpan,
    pub end: TextPos,
}

impl<'a> LiveParser<'a> {
    pub fn new(tokens: &'a [TokenWithSpan], live_type_infos: &'a [LiveTypeInfo], file_id: LiveFileId) -> Self {
        let mut tokens_with_span = tokens.iter().cloned();
        let token_with_span = tokens_with_span.next().unwrap();
        LiveParser {
            live_type_info_counter: 0,
            file_id,
            tokens_with_span,
            live_type_infos,
            token_with_span,
            token_index: 0,
            end: TextPos::default(),
        }
    }
}

impl<'a> LiveParser<'a> {
    
    fn peek_span(&self) -> TextSpan {
        self.token_with_span.span
    }
    
    fn peek_token(&self) -> LiveToken {
        self.token_with_span.token.clone()
    }
    
    fn eat_token(&mut self) -> LiveToken {
        let token = self.peek_token();
        self.skip_token();
        token
    }
    
    fn skip_token(&mut self) {
        self.end = self.token_with_span.span.end;
        self.token_with_span = self.tokens_with_span.next().unwrap();
        self.token_index += 1;
    }
    
    fn error(&mut self, message: String, origin: LiveErrorOrigin) -> LiveError {
        LiveError {
            origin,
            span: self.token_with_span.span.into(),
            message,
        }
    }
    
    
    fn end(&self) -> TextPos {
        self.end
    }
    
    fn token_end(&self) -> TextPos {
        self.token_with_span.span.end
    }
    
    fn accept_ident(&mut self) -> Option<LiveId> {
        if let LiveToken::Ident(id) = self.peek_token() {
            self.skip_token();
            Some(id)
        }
        else {
            None
        }
    }
    
    fn accept_token(&mut self, token: LiveToken) -> bool {
        if self.peek_token() != token {
            return false;
        }
        self.skip_token();
        true
    }
    
    fn expect_ident(&mut self) -> Result<LiveId, LiveError> {
        match self.peek_token() {
            LiveToken::Ident(ident) => {
                self.skip_token();
                Ok(ident)
            }
            token => Err(self.error(format!("expected ident, unexpected token `{}`", token), live_error_origin!())),
        }
    }
    
    fn expect_string(&mut self) -> Result<Rc<String>, LiveError> {
        match self.peek_token() {
            LiveToken::String(v) => {
                self.skip_token();
                Ok(v)
            }
            token => Err(self.error(format!("expected ident, unexpected token `{}`", token), live_error_origin!())),
        }
    }
    
    fn expect_int2(&mut self) -> Result<i64, LiveError> {
        match self.peek_token() {
            LiveToken::Int(v) => {
                self.skip_token();
                Ok(v)
            }
            token => Err(self.error(format!("expected int, unexpected token `{}`", token), live_error_origin!())),
        }
    }
    
    fn expect_float(&mut self) -> Result<f64, LiveError> {
        let sign = if let LiveToken::Punct(live_id!(-)) = self.peek_token(){
            self.skip_token();
            -1.0
        }
        else{
            1.0
        };
        match self.peek_token() {
            LiveToken::Float(v) => {
                self.skip_token();
                Ok(v*sign)
            }
            LiveToken::Int(v) => {
                self.skip_token();
                Ok(v as f64 * sign)
            }
            token => Err(self.error(format!("expected float, unexpected token `{}`", token), live_error_origin!())),
        }
    }
    
    fn expect_token(&mut self, expected: LiveToken) -> Result<(), LiveError> {
        let actual = self.peek_token();
        if actual != expected {
            return Err(self.error(format!("expected {} unexpected token `{}`", expected, actual), live_error_origin!()));
        }
        self.skip_token();
        Ok(())
    }
    
    fn expect_import(&mut self, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        let token_id = self.get_token_id();
        let crate_id = self.expect_ident() ?;
        // if crate_id is capitalized, its a component.
        // so we should make a LiveValue::UseComponent
        self.expect_token(LiveToken::Punct(live_id!(::))) ?;
        
        // ok so. we need to collect everything 'upto' the last id
        let first_module_id = self.expect_ident() ?;
        self.expect_token(LiveToken::Punct(live_id!(::))) ?;
        let mut module = String::new();
        let mut last_id = LiveId(0);
        
        module.push_str(&format!("{}", first_module_id));
        loop {
            match self.peek_token() {
                LiveToken::Ident(id) => {
                    self.skip_token();
                    last_id = id;
                    if !self.accept_token(LiveToken::Punct(live_id!(::))) {
                        break;
                    }
                    module.push_str(&format!("::{}", id));
                },
                LiveToken::Punct(live_id!(*)) => {
                    self.skip_token();
                    last_id = LiveId(0);
                    break;
                }
                _ => {
                    break;
                }
            }
        }
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Instance),
            id: last_id,
            value: LiveValue::Import(LiveModuleId(crate_id, LiveId::from_str(&module).unwrap()))
        });
        
        Ok(())
    }
    /*
   fn expect_registry(&mut self, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        let token_id = self.get_token_id();
        let crate_id = self.expect_ident() ?;
        // if crate_id is capitalized, its a component.
        // so we should make a LiveValue::UseComponent
        self.expect_token(LiveToken::Punct(live_id!(::))) ?;
    
        let component_id = if self.accept_token(LiveToken::Punct(live_id!(*))) {
            LiveId(0)
        }
        else {
            self.expect_ident() ?
        };
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Instance),
            id: component_id,
            value: LiveValue::Registry(crate_id)
        });
        Ok(())
    }*/
    
    fn expect_fn(&mut self, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        let token_start = self.token_index - 1;
        let token_id = self.get_token_id();
        let prop_id = self.expect_ident() ?;
        //let token_start = self.token_index;
        let token_index = self.scan_to_token(LiveToken::Close(Delim::Brace)) ?;
        
        ld.nodes.push(LiveNode {
            origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Instance),
            id: prop_id,
            value: LiveValue::DSL {
                token_start: token_start as u32,
                token_count: (token_index - token_start) as u32,
                expand_index: None
            }
        });
        
        Ok(())
    }
    
    
    fn possible_edit_info(&mut self, ld: &mut LiveOriginal) -> Result<Option<LiveEditInfo>, LiveError> {
        // metadata is .{key:value}
        // lets align to the next index
        if self.accept_token(LiveToken::Punct(live_id!(.))) {
            self.expect_token(LiveToken::Open(Delim::Brace)) ?;
            
            let fill_index = ld.edit_info.len() & 0xf;
            if fill_index != 0 {
                for _ in 0..(16 - fill_index) {
                    ld.edit_info.push(LiveNode::empty())
                }
            }
            let edit_info_index = ld.edit_info.len();
            
            if edit_info_index > 0x3e0 {
                return Err(self.error("Used more than 64 .{..} edit info fields in a file, we dont have the bitspace for that in our u64.".to_string(), live_error_origin!()))
            }
            
            ld.edit_info.push(LiveNode {
                origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                id: LiveId::empty(),
                value: LiveValue::Object
            });
            
            
            while self.peek_token() != LiveToken::Eof {
                match self.peek_token() {
                    LiveToken::Close(Delim::Brace) => {
                        self.skip_token();
                        ld.edit_info.push(LiveNode {
                            origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                            id: LiveId::empty(),
                            value: LiveValue::Close
                        });
                        return Ok(Some(LiveEditInfo::new(edit_info_index)));
                    }
                    LiveToken::Ident(prop_id) => {
                        self.skip_token();
                        self.expect_token(LiveToken::Punct(live_id!(:))) ?;
                        
                        match self.peek_token() {
                            LiveToken::Bool(val) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::Bool(val)
                                });
                            },
                            LiveToken::Int(val) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::Int64(val)
                                });
                            },
                            LiveToken::Float(val) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::Float64(val)
                                });
                            },
                            LiveToken::Color(val) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::Color(val)
                                });
                            },
                            LiveToken::String(rcstring) => {
                                self.skip_token();
                                ld.edit_info.push(LiveNode {
                                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                                    id: prop_id,
                                    value: LiveValue::String(rcstring)
                                });
                            },
                            other => return Err(self.error(format!("Unexpected token {} in edit_info", other), live_error_origin!()))
                        }
                        self.accept_optional_delim();
                    },
                    other => return Err(self.error(format!("Unexpected token {} in edit_info", other), live_error_origin!()))
                }
            }
            return Err(self.error("Eof in edit info".to_string(), live_error_origin!()))
        }
        
        Ok(None)
    }
    
    fn expect_node_with_prefix(&mut self, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        let token_id = self.get_token_id();
        let real_prop_id = self.expect_ident() ?;
        let edit_info = self.possible_edit_info(ld) ?;
        let origin = LiveNodeOrigin::from_token_id(token_id).with_edit_info(edit_info).with_node_has_prefix(true);
        
        if self.accept_token(LiveToken::Punct(live_id!(:))) {
            self.expect_live_value(real_prop_id, origin.with_prop_type(LivePropType::Field), ld) ?;
        }
        else if self.accept_token(LiveToken::Punct(live_id!(=))) {
            self.expect_live_value(real_prop_id, origin.with_prop_type(LivePropType::Instance), ld) ?;
        }
        
        Ok(())
    }
    
    fn expect_array(&mut self, prop_id: LiveId, origin: LiveNodeOrigin, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        self.expect_token(LiveToken::Open(Delim::Bracket)) ?;
        ld.nodes.push(LiveNode {
            origin,
            id: prop_id,
            value: LiveValue::Array
        });
        let mut counter = 1;
        while self.peek_token() != LiveToken::Eof {
            if self.accept_token(LiveToken::Close(Delim::Bracket)) {
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                    id: LiveId::empty(),
                    value: LiveValue::Close
                });
                return Ok(())
            }
            self.expect_live_value(
                LiveId(counter),
                LiveNodeOrigin::from_token_id(self.get_token_id()).with_prop_type(LivePropType::Nameless),
                ld
            ) ?;
            self.accept_token(LiveToken::Punct(live_id!(,)));
            counter += 1;
        }
        Err(self.error("Eof in array body".to_string(), live_error_origin!()))
    }
    
    
    fn expect_tuple_enum(&mut self, prop_id: LiveId, origin: LiveNodeOrigin, variant: LiveId, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        self.expect_token(LiveToken::Open(Delim::Paren)) ?;
        ld.nodes.push(LiveNode {
            origin,
            id: prop_id,
            value: LiveValue::TupleEnum(variant)
        });
        while self.peek_token() != LiveToken::Eof {
            if self.accept_token(LiveToken::Close(Delim::Paren)) {
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::Close
                });
                return Ok(())
            }
            //let span = self.begin_span();
            self.expect_live_value(LiveId::empty(), LiveNodeOrigin::from_token_id(self.get_token_id()).with_prop_type(LivePropType::Nameless), ld) ?;
            self.accept_token(LiveToken::Punct(live_id!(,)));
        }
        Err(self.error("Eof in object body".to_string(), live_error_origin!()))
    }
    
    
    fn expect_named_enum(&mut self, prop_id: LiveId, origin: LiveNodeOrigin,variant: LiveId, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        self.expect_token(LiveToken::Open(Delim::Brace)) ?;
        
        ld.nodes.push(LiveNode {
            origin,
            id: prop_id,
            value: LiveValue::NamedEnum(variant)
        });
        
        while self.peek_token() != LiveToken::Eof {
            if self.accept_token(LiveToken::Close(Delim::Brace)) {
                ld.nodes.push(LiveNode {
                    origin: LiveNodeOrigin::from_token_id(self.get_token_id()),
                    id: prop_id,
                    value: LiveValue::Close
                });
                return Ok(())
            }
            let token_id = self.get_token_id();
            let prop_id = self.expect_ident() ?;
            let edit_info = self.possible_edit_info(ld) ?;
            self.expect_token(LiveToken::Punct(live_id!(:))) ?;
            self.expect_live_value(prop_id, LiveNodeOrigin::from_token_id(token_id).with_edit_info(edit_info), ld) ?;
            self.accept_token(LiveToken::Punct(live_id!(,)));
        }
        Err(self.error("Eof in named enum".to_string(), live_error_origin!()))
    }
    
    fn get_token_id(&self) -> LiveTokenId {
        LiveTokenId::new(self.file_id, self.token_index)
    }
    
    fn expect_live_value(&mut self, prop_id: LiveId, origin: LiveNodeOrigin, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        // now we can have an array or a class instance
        match self.peek_token() {
            LiveToken::Punct(live_id!(<))=>{ // class instance
                self.skip_token();
                
                let ident = self.expect_ident()?;
                self.expect_token(LiveToken::Punct(live_id!(>))) ?;
                self.expect_token(LiveToken::Open(Delim::Brace))?;
                ld.nodes.push(LiveNode {
                    origin,
                    id: prop_id,
                    value: LiveValue::Clone(ident)
                });
                self.expect_live_class(false, prop_id, ld) ?;
            }
            LiveToken::Ident(live_id!(struct)) => { // key/value map
                self.skip_token();
                self.expect_token(LiveToken::Open(Delim::Brace)) ?;
                ld.nodes.push(LiveNode {
                    origin,
                    id: prop_id,
                    value: LiveValue::Clone(live_id!(struct))
                });
                self.expect_live_class(false, prop_id, ld) ?;
            },
            
            LiveToken::Open(Delim::Brace) => { // key/value map
                self.skip_token();
                
                // if we get an OpenBrace immediately after, we are a rust_type
                if self.peek_token() == LiveToken::Open(Delim::Brace) {
                    self.skip_token();
                    
                    let val = self.live_type_info_counter;
                    self.live_type_info_counter += 1;
                    
                    self.accept_ident();
                    
                    if val >= self.live_type_infos.len() {
                        return Err(self.error(format!("live_type index out of range {}", val), live_error_origin!()));
                    }
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Class {
                            live_type: self.live_type_infos[val].live_type,
                            class_parent: None,
                        }
                    });
                    self.expect_token(LiveToken::Close(Delim::Brace)) ?;
                    self.expect_token(LiveToken::Close(Delim::Brace)) ?;
                    
                    self.expect_token(LiveToken::Open(Delim::Brace)) ?;
                    self.expect_live_class(false, prop_id, ld) ?;
                    
                    return Ok(());
                }
                
                ld.nodes.push(LiveNode {
                    origin,
                    id: prop_id,
                    value: LiveValue::Object
                });
                self.expect_live_class(false, prop_id, ld) ?;
            },
            LiveToken::Open(Delim::Paren) => { // expression
                self.expect_expression(prop_id, origin, ld) ?;
            },
            LiveToken::Open(Delim::Bracket) => { // array
                self.expect_array(prop_id, origin, ld) ?;
            },
            LiveToken::Bool(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin,
                    id: prop_id,
                    value: LiveValue::Bool(val)
                });
            },
            LiveToken::Punct(live_id!(-))=>{
                self.skip_token();
                match self.peek_token() {
                    LiveToken::Int(val) => {
                        self.skip_token();
                        ld.nodes.push(LiveNode {
                            origin,
                            id: prop_id,
                            value: LiveValue::Int64(-val)
                        });
                    },
                    LiveToken::Float(val) => {
                        self.skip_token();
                        ld.nodes.push(LiveNode {
                            origin,
                            id: prop_id,
                            value: LiveValue::Float64(-val)
                        });
                    },
                    _=>return Err(self.error("Expected int or float after -".to_string(), live_error_origin!()))
                }
            }
            LiveToken::Int(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin,
                    id: prop_id,
                    value: LiveValue::Int64(val)
                });
            },
            LiveToken::Float(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin,
                    id: prop_id,
                    value: LiveValue::Float64(val)
                });
            },
            LiveToken::Color(val) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin,
                    id: prop_id,
                    value: LiveValue::Color(val)
                });
            },
            LiveToken::String(v) => {
                self.skip_token();
                ld.nodes.push(LiveNode {
                    origin,
                    id: prop_id,
                    value: LiveValue::String(v)
                });
            },
            LiveToken::Ident(live_id!(dep)) => {
                self.skip_token();
                if self.accept_token(LiveToken::Open(Delim::Paren)) {
                    let x = self.expect_string() ?;
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Dependency(x)
                    });
                    self.expect_token(LiveToken::Close(Delim::Paren)) ?;
                }
                else {
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Id(live_id!(dep))
                    });
                }
            }
            LiveToken::Ident(live_id!(vec2)) => {
                self.skip_token();
                if self.accept_token(LiveToken::Open(Delim::Paren)) {
                    let x = self.expect_float() ?;
                    self.expect_token(LiveToken::Punct(live_id!(,))) ?;
                    let y = self.expect_float() ?;
                    self.expect_token(LiveToken::Close(Delim::Paren)) ?;
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Vec2(Vec2 {x: x as f32, y: y as f32})
                    });
                }
                else {
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Id(live_id!(vec2))
                    });
                }
            },
            LiveToken::Ident(live_id!(vec3)) => {
                self.skip_token();
                if self.accept_token(LiveToken::Open(Delim::Paren)) {
                    let x = self.expect_float() ?;
                    self.expect_token(LiveToken::Punct(live_id!(,))) ?;
                    let y = self.expect_float() ?;
                    self.expect_token(LiveToken::Punct(live_id!(,))) ?;
                    let z = self.expect_float() ?;
                    self.expect_token(LiveToken::Close(Delim::Paren)) ?;
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Vec3(Vec3 {x: x as f32, y: y as f32, z: z as f32})
                    });
                }
                else {
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Id(live_id!(vec3))
                    });
                }
            },
            LiveToken::Ident(live_id!(vec4)) => {
                self.skip_token();
                if self.accept_token(LiveToken::Open(Delim::Paren)) {
                    let x = self.expect_float() ?;
                    self.expect_token(LiveToken::Punct(live_id!(,))) ?;
                    let y = self.expect_float() ?;
                    self.expect_token(LiveToken::Punct(live_id!(,))) ?;
                    let z = self.expect_float() ?;
                    self.expect_token(LiveToken::Punct(live_id!(,))) ?;
                    let w = self.expect_float() ?;
                    self.expect_token(LiveToken::Close(Delim::Paren)) ?;
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Vec4(Vec4 {x: x as f32, y: y as f32, z: z as f32, w: w as f32})
                    });
                }
                else {
                    ld.nodes.push(LiveNode {
                        origin,
                        id: prop_id,
                        value: LiveValue::Id(live_id!(vec4))
                    });
                }
            },
            LiveToken::Ident(variant) => { // parse enum
                self.skip_token();
                match self.peek_token() {
                    LiveToken::Punct(live_id!(.))=>{ // IdPath
                        let mut id_path = Vec::new();
                        id_path.push(variant);
                        while self.accept_token(LiveToken::Punct(live_id!(.))){
                            let ident = self.expect_ident()?;
                            id_path.push(ident);
                        }
                        ld.nodes.push(LiveNode {
                            origin,
                            id: prop_id,
                            value: LiveValue::IdPath(Rc::new(id_path))
                        })
                        // loop till not id or =>
                    }
                    LiveToken::Open(Delim::Brace) => {
                        self.expect_named_enum(prop_id, origin, variant, ld) ?;
                    }
                    LiveToken::Open(Delim::Paren) => {
                        self.expect_tuple_enum(prop_id, origin, variant, ld) ?;
                    }
                    _ => {
                        if variant.as_string(|v| v.unwrap().chars().next().unwrap().is_uppercase()){
                            ld.nodes.push(LiveNode {
                                origin,
                                id: prop_id,
                                value: LiveValue::BareEnum(variant)
                            })
                        }
                        else{
                            ld.nodes.push(LiveNode {
                                origin,
                                id: prop_id,
                                value: LiveValue::Id(variant)
                            })
                        }
                    }
                }
            },
            other => return Err(self.error(format!("Unexpected token {} in property value", other), live_error_origin!()))
        }
        Ok(())
    }
    
    fn scan_to_token(&mut self, scan_token: LiveToken) -> Result<usize, LiveError> {
        // ok we are going to scan to token, keeping in mind our levels.
        let mut stack_depth = 0;
        
        while self.peek_token() != LiveToken::Eof {
            match self.peek_token() {
                LiveToken::Open(_) => {
                    stack_depth += 1;
                }
                LiveToken::Close(_) => {
                    if stack_depth == 0 {
                        return Err(self.error(format!("Found closing )}}] whilst scanning for {}", scan_token), live_error_origin!()));
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
        Err(self.error(format!("Could not find ending token {} whilst scanning", scan_token), live_error_origin!()))
    }
    /*
    fn expect_var_def_type(&mut self) -> Result<(), LiveError> {
        self.expect_ident() ?;
        if self.accept_token(LiveToken::Ident(live_id!(in))) {
            self.expect_ident() ?;
        }
        Ok(())
    }*/
    
    fn expect_value_literal(&mut self) -> Result<(), LiveError> {
        match self.peek_token() {
            LiveToken::Bool(_)
                | LiveToken::Int(_)
                | LiveToken::Float(_)
                | LiveToken::Color(_) => {
                self.skip_token();
                return Ok(())
            }
            LiveToken::Ident(live_id!(vec2)) => {todo!()}
            LiveToken::Ident(live_id!(vec3)) => {todo!()}
            _ => ()
        }
        Err(self.error("Expected value literal".to_string(), live_error_origin!()))
    }
    
    fn expect_live_class(&mut self, root: bool, prop_id: LiveId, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        
        while self.peek_token() != LiveToken::Eof {
            match self.peek_token() {
                LiveToken::Close(Delim::Brace) => {
                    if root {
                        return Err(self.error("Unexpected token } in root".to_string(), live_error_origin!()))
                    }
                    let token_id = self.get_token_id();
                    self.skip_token();
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id),
                        id: prop_id,
                        value: LiveValue::Close
                    });
                    return Ok(());
                }
                LiveToken::Punct(live_id!(<))=>{ // class instance
                    self.skip_token();
                    let token_id = self.get_token_id();
                    let ident = self.expect_ident()?;
                    self.expect_token(LiveToken::Punct(live_id!(>))) ?;
                    self.expect_token(LiveToken::Open(Delim::Brace))?;
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Instance),
                        id: LiveId::unique(),
                        value: LiveValue::Clone(ident)
                    });
                    self.expect_live_class(false, prop_id, ld) ?;
                }
                LiveToken::Ident(prop_id) => {
                    let token_id = self.get_token_id();
                    self.skip_token();
                    
                    //let span = self.begin_span();
                    // next
                    // there is another token coming
                    if let LiveToken::Ident(_) = self.peek_token() {
                        match prop_id {
                            live_id!(fn) => {
                                self.expect_fn(ld) ?;
                                self.accept_optional_delim();
                            }
                            live_id!(import) => {
                                self.expect_import(ld) ?;
                                self.accept_optional_delim();
                            }
                            _ => {
                                let token_id = self.get_token_id();
                                let real_prop_id = self.expect_ident() ?;
                                let edit_info = self.possible_edit_info(ld) ?;
                                let prop_type = self.expect_prop_type()?;
                                let origin = LiveNodeOrigin::from_token_id(token_id)
                                    .with_edit_info(edit_info)
                                    .with_node_has_prefix(true)
                                    .with_prop_type(prop_type);
                                
                                self.expect_live_value(real_prop_id, origin, ld) ?;
                                //self.expect_node_with_prefix(ld) ?;
                                self.accept_optional_delim();
                            }
                        }
                    }
                    else { // has to be key:value
                        // if we get a . metadata follows
                        let edit_info = self.possible_edit_info(ld) ?;
                  
                        let assign_type = self.expect_prop_type()?;
                        let origin = LiveNodeOrigin::from_token_id(token_id)
                            .with_edit_info(edit_info)
                            .with_prop_type(assign_type);
                        self.expect_live_value(prop_id, origin, ld) ?;

                        self.accept_optional_delim();
                    }
                },
                other => return Err(self.error(format!("Unexpected token {} in class body of {}", other, prop_id), live_error_origin!()))
            }
        }
        if root {
            return Ok(())
        }
        Err(self.error("Eof in class body".to_string(), live_error_origin!()))
    }
    
    pub fn expect_prop_type(&mut self)->Result<LivePropType, LiveError>{
        Ok(if self.accept_token(LiveToken::Punct(live_id!(:))){
            LivePropType::Field
        }
        else if self.accept_token(LiveToken::Punct(live_id!(=))){
            /*if self.accept_token(LiveToken::Punct(live_id!( ?))) {
                LivePropType::Template
            }
            else{*/
                LivePropType::Instance
            //}
        }
        else{
            return Err(self.error("Unexpected assign_type, expected = or :".to_string(), live_error_origin!()))
        })
    }
    
    pub fn accept_optional_delim(&mut self) {
        if !self.accept_token(LiveToken::Punct(live_id!(,))) {
            self.accept_token(LiveToken::Punct(live_id!(;)));
        }
    }
    
    pub fn parse_live_document(&mut self) -> Result<LiveOriginal, LiveError> {
        let mut ld = LiveOriginal::new();
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
    
    
    
    
    fn expect_expression(&mut self, prop_id: LiveId, origin: LiveNodeOrigin, ld: &mut LiveOriginal) -> Result<(), LiveError> {
        
        let expr = self.expect_prim_expr() ?;
        
        ld.nodes.push(LiveNode {
            origin,
            id: prop_id,
            value: LiveValue::Expr {expand_index: None}
        });
        
        fn recur_walk(expr: Expr, ld: &mut LiveOriginal) {
            match expr {
                Expr::Bin {token_id, op, left_expr, right_expr} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
                        id: LiveId::empty(),
                        value: LiveValue::ExprBinOp(op)
                    });
                    recur_walk(*left_expr, ld);
                    recur_walk(*right_expr, ld);
                }
                Expr::Un {token_id, op, expr} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
                        id: LiveId::empty(),
                        value: LiveValue::ExprUnOp(op)
                    });
                    recur_walk(*expr, ld);
                }
                Expr::Call {token_id, ident, arg_exprs} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
                        id: LiveId::empty(),
                        value: LiveValue::ExprCall {ident, args: arg_exprs.len()}
                    });
                    for arg in arg_exprs {
                        recur_walk(arg, ld);
                    }
                }
                Expr::Member {token_id, ident, expr} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
                        id: LiveId::empty(),
                        value: LiveValue::ExprMember(ident)
                    });
                    recur_walk(*expr, ld);
                }
                Expr::Var {token_id, ident} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
                        id: LiveId::empty(),
                        value: LiveValue::Id(ident)
                    });
                }
                Expr::Bool {token_id, v} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
                        id: LiveId::empty(),
                        value: LiveValue::Bool(v)
                    });
                }
                Expr::Int {token_id, v} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
                        id: LiveId::empty(),
                        value: LiveValue::Int64(v)
                    });
                }
                Expr::Float {token_id, v} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
                        id: LiveId::empty(),
                        value: LiveValue::Float64(v)
                    });
                }
                Expr::Color {token_id, v} => {
                    ld.nodes.push(LiveNode {
                        origin: LiveNodeOrigin::from_token_id(token_id).with_prop_type(LivePropType::Nameless),
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
            self.accept_token(Token::Punct(live_id!(,)));
        }
        return Err(self.error(format!("Eof in array body")))*/
    }
    
    fn expect_expr(&mut self) -> Result<Expr, LiveError> {
        self.expect_cond_expr()
    }
    
    fn expect_cond_expr(&mut self) -> Result<Expr, LiveError> {
        let expr = self.expect_or_expr() ?;
        Ok(if self.accept_token(LiveToken::Punct(live_id!( ?))) {
            let token_id = self.get_token_id();
            let expr_if_true = self.expect_expr() ?;
            self.expect_token(LiveToken::Punct(live_id!(:))) ?;
            let expr_if_false = self.expect_cond_expr() ?;
            Expr ::Call {token_id, ident: live_id!(cond), arg_exprs: vec![expr, expr_if_true, expr_if_false]}
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
        let mut acc = self.expect_prim_expr()?;
        while let LiveToken::Punct(live_id!(.)) = self.peek_token() {
            self.skip_token();
            let token_id = self.get_token_id();
            let ident = self.expect_ident()?;
            acc = Expr::Member {token_id, ident, expr: Box::new(acc)}
        }
        Ok(acc)
    }
    
    fn expect_prim_expr(&mut self) -> Result<Expr, LiveError> {
        match self.peek_token() {
            LiveToken::Ident(ident) => {
                let token_id = self.get_token_id();
                self.skip_token();
                match self.peek_token() {
                    LiveToken::Open(Delim::Paren) => {
                        let arg_exprs = self.expect_arg_exprs() ?;
                        Ok(Expr::Call {token_id, ident, arg_exprs})
                    }
                    _ => Ok(Expr::Var {token_id, ident}),
                }
            }
            LiveToken::Bool(v) => {
                self.skip_token();
                let token_id = self.get_token_id();
                Ok(Expr::Bool {token_id, v})
            }
            LiveToken::Int(v) => {
                self.skip_token();
                let token_id = self.get_token_id();
                Ok(Expr::Int {token_id, v})
            }
            LiveToken::Float(v) => {
                self.skip_token();
                let token_id = self.get_token_id();
                Ok(Expr::Float {token_id, v})
            }
            LiveToken::Color(v) => {
                self.skip_token();
                let token_id = self.get_token_id();
                Ok(Expr::Color {token_id, v})
            }
            LiveToken::Open(Delim::Paren) => {
                self.skip_token();
                let expr = self.expect_expr() ?;
                self.expect_token(LiveToken::Close(Delim::Paren)) ?;
                Ok(expr)
            }
            token => Err(self.error(format!("Unexpected token {} in class expression", token), live_error_origin!()))
        }
    }
    
    fn expect_arg_exprs(&mut self) -> Result<Vec<Expr>, LiveError> {
        self.expect_token(LiveToken::Open(Delim::Paren)) ?;
        let mut arg_exprs = Vec::new();
        if !self.accept_token(LiveToken::Close(Delim::Paren)) {
            loop {
                arg_exprs.push(self.expect_expr() ?);
                if !self.accept_token(LiveToken::Punct(live_id!(,))) {
                    break;
                }
            }
            self.expect_token(LiveToken::Close(Delim::Paren)) ?;
        }
        Ok(arg_exprs)
    }
    
    
}

#[derive(Debug)]
enum Expr {
    Bin {
        token_id: LiveTokenId,
        op: LiveBinOp,
        left_expr: Box<Expr>,
        right_expr: Box<Expr>,
    },
    Un {
        token_id: LiveTokenId,
        op: LiveUnOp,
        expr: Box<Expr>,
    },
    Call {
        token_id: LiveTokenId,
        ident: LiveId,
        arg_exprs: Vec<Expr>,
    },
    Member {
        token_id: LiveTokenId,
        ident: LiveId,
        expr: Box<Expr>
    },
    Var {
        token_id: LiveTokenId,
        ident: LiveId,
    },
    Bool {
        token_id: LiveTokenId,
        v: bool
    },
    Int {
        token_id: LiveTokenId,
        v: i64
    },
    Float {
        token_id: LiveTokenId,
        v: f64
    },
    Color {
        token_id: LiveTokenId,
        v: u32
    }
}


impl LiveBinOp {
    fn from_or_op(token: LiveToken) -> Option<Self> {
        match token {
            LiveToken::Punct(live_id!( ||)) => Some(Self::Or),
            _ => None,
        }
    }
    
    fn from_and_op(token: LiveToken) -> Option<Self> {
        match token {
            LiveToken::Punct(live_id!( &&)) => Some(Self::And),
            _ => None,
        }
    }
    
    fn from_eq_op(token: LiveToken) -> Option<Self> {
        match token {
            LiveToken::Punct(live_id!( ==)) => Some(Self::Eq),
            LiveToken::Punct(live_id!( !=)) => Some(Self::Ne),
            _ => None,
        }
    }
    
    fn from_rel_op(token: LiveToken) -> Option<Self> {
        match token {
            LiveToken::Punct(live_id!(<)) => Some(Self::Lt),
            LiveToken::Punct(live_id!( <=)) => Some(Self::Le),
            LiveToken::Punct(live_id!(>)) => Some(Self::Gt),
            LiveToken::Punct(live_id!( >=)) => Some(Self::Ge),
            _ => None,
        }
    }
    
    fn from_add_op(token: LiveToken) -> Option<Self> {
        match token {
            LiveToken::Punct(live_id!( +)) => Some(Self::Add),
            LiveToken::Punct(live_id!(-)) => Some(Self::Sub),
            _ => None,
        }
    }
    
    fn from_mul_op(token: LiveToken) -> Option<Self> {
        match token {
            LiveToken::Punct(live_id!(*)) => Some(Self::Mul),
            LiveToken::Punct(live_id!( /)) => Some(Self::Div),
            _ => None,
        }
    }
}

impl LiveUnOp {
    pub fn from_un_op(token: LiveToken) -> Option<Self> {
        match token {
            LiveToken::Punct(live_id!(!)) => Some(Self::Not),
            LiveToken::Punct(live_id!(-)) => Some(Self::Neg),
            _ => None,
        }
    }
}

