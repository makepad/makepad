use crate::error::LiveError;
use crate::lit::{Lit};
use crate::ident::{Ident};
use crate::token::{Token, TokenWithSpan};
use crate::livetypes::*;
use crate::detok::*;
use std::collections::HashSet;
use crate::livestyles::{LiveTokensType, LiveTokens};

impl<'a> DeTokParserImpl<'a>{
    
    pub fn parse_float(&mut self)->Result<Float, LiveError>{
        return Ok(Float::default());
    } 
    
    fn parse_block_tokens(&mut self, live_id: LiveId) -> Result<Vec<TokenWithSpan>, LiveError> { // scans from { to }
        self.clear_token_storage();
        let mut paren_stack = Vec::new();
        let mut new_deps = HashSet::new();
        
        while self.peek_token() != Token::Eof {
            match self.peek_token() {
                Token::Ident(_) => {
                    let ident_path = self.parse_ident_path() ?;
                    if ident_path.len()>1 {
                        let qualified_ident_path = self.qualify_ident_path(&ident_path);
                        let on_live_id = qualified_ident_path.to_live_id();
                        // lets query if this one somehow depends on me
                        if self.live_styles.check_depends_on(live_id, on_live_id){
                            // cyclic dep
                            return Err(self.error(format!("Cyclic dependency {}",ident_path)))
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
                        // lets remove old deps..
                        self.live_styles.update_deps(live_id, new_deps);
                        return Ok(self.get_token_storage());
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
        
        let live_body_id = self.peek_span().live_body_id;
        
        while self.peek_token() != Token::Eof {
            let span = self.begin_span();
            
            // at this level we expect a live_id
            let ident_path = self.parse_ident_path() ?;
            let qualified_ident_path = self.qualify_ident_path(&ident_path);
            let live_id = qualified_ident_path.to_live_id();
            
            new_body_contains.insert(live_id);
            
            if let Some(lstok) = self.live_styles.tokens.get(&live_id) {
                if lstok.qualified_ident_path != qualified_ident_path {
                    let msg = format!("Ident live_id hash collision between `{}` and `{}` rename one of them", lstok.qualified_ident_path, qualified_ident_path);
                    return Err(span.error(self, msg));
                }
            }
            
            self.expect_token(Token::Colon) ?;
            
            match self.peek_token() {
                Token::Ident(ident) => {
                    self.skip_token();
                    let tokens = self.parse_block_tokens(live_id) ?;
                    // see if the tokens changed, ifso mark this thing dirty
                    self.live_styles.add_recompute_when_tokens_different(live_id, &tokens);
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: {
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
                        }
                    });
                }
                /*
                Token::Ident(ident) if ident == Ident::new("Style") => {
                    // its a style.
                    self.parser.skip_token();
                    self.parser.expect_token(Token::LeftBrace) ?;
                    // PARSE STYLE FORWARDS
                }
                Token::Ident(ident) if ident == Ident::new("Shader") => {
                    // lets parse this shaaaader!
                    self.parser.skip_token();
                    self.parser.clear_token_storage();
                    // get current token
                    let mut shader_ast = ShaderAst::new();
                    shader_ast.qualified_ident_path = qualified_ident_path;
                    shader_ast.module_path = self.module_path.clone();
                    
                    self.parse_shader(live_id, &mut shader_ast) ?;
                    
                    if let Some(tokens) = self.live_styles.tokens.get(&live_id) {
                        // already exists.
                        // lets check if the type changed
                        // lets walk all deps and clear things
                    }
                    else { // new slot
                        shader_ast.shader = Some(Shader {
                            shader_id: *self.shader_alloc_start,
                            location_hash: 0
                        });
                        *self.shader_alloc_start += 1;
                    }
                    self.live_styles.shaders.insert(live_id, shader_ast);
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens: self.token_storage.clone(),
                        live_tokens_type: LiveTokensType::Shader
                    });
                }
                Token::Ident(ident) if ident == Ident::new("ShaderLib") => {
                    // lets parse this shaaaader!
                    self.skip_token();
                    self.token_storage.truncate(0);
                    // lets make a new shader_ast
                    let mut shader_ast = ShaderAst::new();
                    shader_ast.qualified_ident_path = qualified_ident_path;
                    shader_ast.module_path = self.module_path.clone();
                    self.parse_shader(live_id, &mut shader_ast) ?;
                    self.live_styles.shader_libs.insert(live_id, shader_ast);
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens: self.token_storage.clone(),
                        live_tokens_type: LiveTokensType::ShaderLib
                    });
                }
                Token::Ident(ident) if ident == Ident::new("Anim") => {
                    self.skip_token();
                    let tokens = self.parse_block_tokens(live_id) ?;
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: LiveTokensType::Anim
                    });
                }
                Token::Ident(ident) if ident == Ident::new("Layout") => { // lets parse these things
                    self.skip_token();
                    let tokens = self.parse_block_tokens(live_id) ?;
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: LiveTokensType::Layout
                    });
                }
                Token::Ident(ident) if ident == Ident::new("Walk") => { // lets parse these things
                    self.skip_token();
                    let tokens = self.parse_block_tokens(live_id) ?;
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: LiveTokensType::Walk
                    });
                }
                Token::Ident(ident) if ident == Ident::new("TextStyle") => { // lets parse these things
                    self.skip_token();
                    let tokens = self.parse_block_tokens(live_id) ?;
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: LiveTokensType::TextStyle
                    });
                }*/
                Token::Lit(Lit::Int(_)) | Token::Lit(Lit::Float(_)) => {
                    self.clear_token_storage();
                    self.skip_token();
                    let tokens = self.get_token_storage();
                    self.live_styles.add_recompute_when_tokens_different(live_id, &tokens);
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: LiveTokensType::Float
                    });
                    self.expect_token(Token::Semi) ?;
                }
                Token::Lit(Lit::Color(_)) => {
                    self.clear_token_storage();
                    self.skip_token();
                    //let value = f32::de_tok(self) ?;
                    let tokens = self.get_token_storage();
                    self.live_styles.add_recompute_when_tokens_different(live_id, &tokens);
                    self.live_styles.tokens.insert(live_id, LiveTokens {
                        qualified_ident_path,
                        tokens,
                        live_tokens_type: LiveTokensType::Color
                    });
                    self.expect_token(Token::Semi) ?;
                }
                token => {
                    return Err(span.error(self, format!("Unexpected token {}", token)));
                }
            }
            
        }
        // compare live_body_contains with the old one and cache_clear the missing ones.
        if let Some(live_body_contains) = self.live_styles.live_bodies_contains.get(&live_body_id).cloned() {
            for contains_live_id in live_body_contains {
                if !new_body_contains.contains(&contains_live_id) {
                    self.live_styles.add_recompute_dep(contains_live_id);
                    // temove tokens
                    self.live_styles.tokens.remove(&contains_live_id);
                }
            }
        }
        self.live_styles.live_bodies_contains.insert(live_body_id, new_body_contains);
        
        Ok(())
    }
}
