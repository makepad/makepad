use crate::error::LiveError;
use crate::lit::Lit;
use crate::ty::TyLit;
use crate::ident::{Ident};
use crate::token::{Token, TokenWithSpan};
use crate::livetypes::*;
use crate::detok::*;
use std::collections::HashSet;
use crate::livestyles::{LiveTokensType, LiveStyle, LiveTokens};
use crate::math::*;

impl<'a> DeTokParserImpl<'a> {
    
    pub fn parse_float(&mut self) -> Result<Float, LiveError> {
        // check what we are up against.
        match self.peek_token() {
            Token::Lit(Lit::Int(v)) => {
                return Ok(Float {value: v as f32, ..Float::default()});
            }
            Token::Lit(Lit::Float(v)) => {
                return Ok(Float {value: v, ..Float::default()});
            },
            Token::Ident(ident) if ident == Ident::new("Float") => {
                return Float::de_tok(self)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing float", self.peek_token())))
        }
    }
    
    pub fn parse_vec2(&mut self) -> Result<Vec2, LiveError> {
        // check what we are up against.
        match self.peek_token() {
            Token::TyLit(tylit) if tylit == TyLit::Vec2 => {
                return Vec2::de_tok(self)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing vec2", self.peek_token())))
        }
    }
    
    pub fn parse_vec3(&mut self) -> Result<Vec3, LiveError> {
        // check what we are up against.
        match self.peek_token() {
            Token::TyLit(tylit) if tylit == TyLit::Vec3 => {
                return Vec3::de_tok(self)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing vec3", self.peek_token())))
        }
    }
    
    pub fn parse_vec4(&mut self) -> Result<Vec4, LiveError> {
        // check what we are up against.
        match self.peek_token() {
            Token::TyLit(tylit) if tylit == TyLit::Vec4 => {
                return Vec4::de_tok(self)
            },
            Token::Lit(Lit::Vec4(v))=>{
                return Ok(v)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing vec4", self.peek_token())))
        }
    }
    
    pub fn parse_text_style(&mut self) -> Result<TextStyle, LiveError> {
        // check what we are up against.
        match self.peek_token() {
            Token::Ident(ident) if ident == Ident::new("TextStyle") => {
                return TextStyle::de_tok(self)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing text_style", self.peek_token())))
        }
    }
    
    pub fn parse_layout(&mut self) -> Result<Layout, LiveError> {
        // check what we are up against.
        match self.peek_token() {
            Token::Ident(ident) if ident == Ident::new("Layout") => {
                return Layout::de_tok(self)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing layout", self.peek_token())))
        }
    }
    
    pub fn parse_walk(&mut self) -> Result<Walk, LiveError> {
        // check what we are up against.
        match self.peek_token() {
            Token::Ident(ident) if ident == Ident::new("Walk") => {
                return Walk::de_tok(self)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing walk", self.peek_token())))
        }
    }
    
    pub fn parse_anim(&mut self) -> Result<Anim, LiveError> {
        // check what we are up against.
        match self.peek_token() {
            Token::Ident(ident) if ident == Ident::new("Anim") => {
                return Anim::de_tok(self)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing anim", self.peek_token())))
        }
    }
    
    pub fn parse_style(&mut self) -> Result<LiveStyle, LiveError> {
        match self.peek_token() {
            Token::Ident(ident) if ident == Ident::new("Style") => {
                return LiveStyle::de_tok(self)
            },
            _ => return Err(self.error(format!("Unexpected {} while parsing style", self.peek_token())))
        }
    }
    
    fn parse_block_tokens(&mut self, live_item_id: LiveItemId) -> Result<Vec<TokenWithSpan>, LiveError> { // scans from { to }
        self.clear_token_clone();
        let mut paren_stack = Vec::new();
        let mut new_deps = HashSet::new();
        
        while self.peek_token() != Token::Eof {
            match self.peek_token() {
                Token::Ident(_) => {
                    let ident_path = self.parse_ident_path() ?;
                    if ident_path.len()>1 {
                        let qualified_ident_path = self.qualify_ident_path(&ident_path);
                        let on_live_id = qualified_ident_path.to_live_item_id();
                        // lets query if this one somehow depends on me
                        if self.live_styles.check_depends_on(live_item_id, on_live_id) {
                            return Err(self.error(format!("Cyclic dependency {}", ident_path)))
                        }
                        new_deps.insert(on_live_id);
                    }
                },
                Token::LeftParen => {
                    self.skip_token();
                    paren_stack.push(Token::RightParen);
                },
                Token::LeftBrace => {
                    self.skip_token();
                    paren_stack.push(Token::RightBrace);
                },
                Token::LeftBracket => {
                    self.skip_token();
                    paren_stack.push(Token::RightBracket);
                },
                Token::RightParen | Token::RightBrace | Token::RightBracket => {
                    let token = self.peek_token();
                    if paren_stack.len() == 0 {
                        return Err(self.error(format!("Unexpected {}", token)))
                    }
                    let last = paren_stack.pop().unwrap();
                    if last != token {
                        return Err(self.error(format!("Mismatch, expected {} got {}", last, token)))
                    }
                    self.skip_token();
                    if paren_stack.len() == 0 {
                        self.live_styles.update_deps(live_item_id, new_deps);
                        return Ok(self.get_token_clone());
                    }
                },
                _ => {
                    self.skip_token()
                }
            }
        }
        return Err(self.error(format!("Unexpected eof parsing tokens block")))
    }
    
    // parse a live block into live_tokens
    pub fn parse_live(&mut self) -> Result<(), LiveError> {
        
        let mut new_body_contains = HashSet::new();
        let mut body_items = Vec::new();
        let live_body_id = self.peek_span().live_body_id;
        
        while self.peek_token() != Token::Eof {
            let span = self.begin_span();
            
            // at this level we expect a live_id
            let ident_path = self.parse_ident_path() ?;
            let qualified_ident_path = self.qualify_ident_path(&ident_path);
            let live_item_id = qualified_ident_path.to_live_item_id();
            
            new_body_contains.insert(live_item_id);
            
            self.live_styles.item_in_live_body.insert(live_item_id, live_body_id);
            
            body_items.push(live_item_id);
            
            if let Some(lstok) = self.live_styles.tokens.get(&live_item_id) {
                if lstok.qualified_ident_path != qualified_ident_path {
                    let msg = format!("Ident live_id hash collision between `{}` and `{}` rename one of them", lstok.qualified_ident_path, qualified_ident_path);
                    return Err(span.error(self, msg));
                }
            }
            
            self.expect_token(Token::Colon) ?;
            
            match self.peek_token() {
                Token::TyLit(tylit) => {
                    let tokens = self.parse_block_tokens(live_item_id) ?;
                    let live_tokens_type = match tylit {
                        TyLit::Vec2 => {
                            LiveTokensType::Vec2
                        }
                        TyLit::Vec3 => {
                            LiveTokensType::Vec3
                        }
                        TyLit::Vec4 => {
                            LiveTokensType::Vec4
                        }
                        _ => {
                            return Err(span.error(self, format!("Tylit {} unexpected", tylit)));
                        }
                    };
                    self.live_styles.add_changed_deps(live_item_id, &tokens, live_tokens_type);
                    self.live_styles.tokens.insert(live_item_id, LiveTokens {
                        ident_path,
                        qualified_ident_path,
                        tokens,
                        live_tokens_type
                    });
                    self.expect_token(Token::Semi) ?;
                },
                Token::Ident(ident) => {
                    let tokens = self.parse_block_tokens(live_item_id) ?;
                    // see if the tokens changed, ifso mark this thing dirty
                    let live_tokens_type = {
                        if ident == Ident::new("Float") {
                            LiveTokensType::Float
                        }
                        else if ident == Ident::new("Color") {
                            LiveTokensType::Shader
                        }
                        else if ident == Ident::new("TextStyle") {
                            LiveTokensType::TextStyle
                        }
                        else if ident == Ident::new("Layout") {
                            LiveTokensType::Layout
                        }
                        else if ident == Ident::new("Walk") {
                            LiveTokensType::Walk
                        }
                        else if ident == Ident::new("Anim") {
                            LiveTokensType::Anim
                        }
                        else if ident == Ident::new("Style") {
                            LiveTokensType::Style
                        }
                        else if ident == Ident::new("ShaderLib") {
                            LiveTokensType::ShaderLib
                        }
                        else if ident == Ident::new("Shader") {
                            LiveTokensType::Shader
                        }
                        else {
                            return Err(span.error(self, format!("Ident {} unexpected", ident)));
                        }
                    };
                    self.live_styles.add_changed_deps(live_item_id, &tokens, live_tokens_type);
                    self.live_styles.tokens.insert(live_item_id, LiveTokens {
                        ident_path,
                        qualified_ident_path,
                        tokens,
                        live_tokens_type
                    });
                }
                Token::Lit(Lit::Int(_)) | Token::Lit(Lit::Float(_)) => {
                    self.clear_token_clone();
                    self.skip_token();
                    let tokens = self.get_token_clone();
                    self.live_styles.update_deps(live_item_id, HashSet::new());
                    self.live_styles.add_changed_deps(live_item_id, &tokens, LiveTokensType::Float);
                    self.live_styles.tokens.insert(live_item_id, LiveTokens {
                        ident_path,
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: LiveTokensType::Float
                    });
                    self.expect_token(Token::Semi) ?;
                }
                Token::Lit(Lit::Vec4(_)) => {
                    self.clear_token_clone();
                    self.skip_token();
                    //let value = f32::de_tok(self) ?;
                    let tokens = self.get_token_clone();
                    self.live_styles.update_deps(live_item_id, HashSet::new());
                    self.live_styles.add_changed_deps(live_item_id, &tokens, LiveTokensType::Vec4);
                    self.live_styles.tokens.insert(live_item_id, LiveTokens {
                        ident_path,
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: LiveTokensType::Vec4
                    });
                    self.expect_token(Token::Semi) ?;
                }
                token => {
                    return Err(span.error(self, format!("Unexpected token in parse_live {:?}", token)));
                }
            }
            
        }
        // compare live_body_contains with the old one and cache_clear the missing ones.
        if let Some(live_body_contains) = self.live_styles.live_bodies_contains.get(&live_body_id).cloned() {
            for contains_live_id in live_body_contains {
                if !new_body_contains.contains(&contains_live_id) {
                    if let Some(tokens) = self.live_styles.tokens.get(&contains_live_id){
                        println!("REMOVING ITEM {}", tokens.qualified_ident_path);
                    }
                    self.live_styles.remove_live_id(contains_live_id);
                }
            }
        }
        self.live_styles.live_bodies_contains.insert(live_body_id, new_body_contains);
        self.live_styles.live_bodies_items.insert(live_body_id, body_items);
        Ok(())
    }
}
