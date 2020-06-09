use makepad_widget::*;
use crate::mprstokenizer::*;
use makepad_render::makepad_shader_compiler::ast::*;
use makepad_render::makepad_shader_compiler::emit::Emitter;
use makepad_render::makepad_shader_compiler::lex;
use makepad_render::makepad_shader_compiler::parse::Parser;
use std::error::Error;


pub struct LiveMacroPick {
    token: usize,
}

pub struct LiveMacroShader {
    token: usize,
}

pub enum LiveMacro {
    Pick(LiveMacroPick),
    Shader(LiveMacroShader)
}

#[derive(Default)]
pub struct LiveMacros {
    macros: Vec<LiveMacro>
}

impl LiveMacros {
    pub fn parse(&mut self, text_buffer: &TextBuffer) {
        let mut tp = TokenParser::new(&text_buffer.flat_text, &text_buffer.token_chunks);
        while tp.advance() {
            match tp.cur_type() {
                TokenType::Macro => {
                    if tp.eat("shader") &&
                    tp.eat("!") &&
                    tp.eat("{") {
                        // ok now we are at ", hopefully
                        // get matching pair
                        if tp.cur_type() == TokenType::ParenOpen {
                            let start = tp.next_index;
                            let end = tp.cur_pair_token();
                            
                            // don't jump, there might be actual value macros
                            // in the shader itself
                            if let Err(ref error) = ( || -> Result<(), Box<dyn Error>> {
                                let tokens = lex::lex(
                                    tp.flat_text[start..end].iter().cloned()
                                ).collect::<Result<Vec<_>, _>>() ?;
                                let shader = ParsedShader::parse(&mut Parser::new(tokens.iter().cloned())) ?;
                                let shader_attrs = shader.emit(&mut Emitter::new()) ?;
                                println!("VERTEX SHADER:");
                                println!("{}", shader_attrs.vertex_string);
                                println!("FRAGMENT SHADER:");
                                println!("{}", shader_attrs.fragment_string);
                                Ok(())
                            })() { 
                                println!("{}", error);
                            }
                            
                        }
                    }
                },
                _ => ()
            }
        }
        //let new_tok = &text_buffer.token_chunks[new_index];
        //let new_tok_slice = &text_buffer.flat_text[new_tok.offset..new_tok.offset + new_tok.len];
        //let old_tok = &text_buffer.old_token_chunks[old_index];
        //let old_tok_slice = &text_buffer.flat_text[old_tok.offset..old_tok.offset + old_tok.len];
    }
}