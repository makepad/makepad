use makepad_render::*;
use makepad_widget::*;
use crate::mprstokenizer::*;
use crate::appstorage::*;

pub struct LiveMacroPick {
    _token: usize,
}

pub struct LiveMacroShader {
    _token: usize,
}

pub enum LiveMacro {
    Pick(LiveMacroPick),
    Shader(LiveMacroShader)
}

#[derive(Default)]
pub struct LiveMacros {
    _macros: Vec<LiveMacro>
}

impl AppTextBuffer {
    pub fn parse_live_macros(&mut self, cx: &mut Cx) {
        let mut tp = TokenParser::new(&self.text_buffer.flat_text, &self.text_buffer.token_chunks);
        while tp.advance() {
            match tp.cur_type() {
                TokenType::Macro => {
                    if tp.eat("shader") &&
                    tp.eat("!") &&
                    tp.eat("{") {
                        if tp.cur_type() == TokenType::ParenOpen {
                            
                            // lets get the filename, the line
                            // lets slice out the whole shader
                            // then we hand it to the render backend
                            // to diff it
                            // lets fetch a range
                            // lets get the linenr
                            
                            if let Some(shader) = tp.cur_pair_as_string() {
                                let lc = tp.cur_line_col();
                                
                                cx.recompile_shader_sub(
                                    &self.full_path["main/makepad/".len()..],
                                    lc.0 + 1,
                                    lc.1 - 8,
                                    shader
                                );
                                //println!("{} {}:{}",self.full_path, lc.0, lc.1);
                            }
                            
                            tp.jump_to_pair();
                            // ok now we are at ", hopefully
                            // get matching pair
                            // let start = tp.next_index;
                            // we find the right slot in the shader
                            //
                            /*
                            let end = tp.cur_pair_token();
                            
                            // don't jump, there might be actual value macros
                            // in the shader itself
                            
                            if let Err(ref error) = ( || -> Result<(), Box<dyn Error>> {
                                let tokens = lex::lex(
                                    tp.flat_text[start..end].iter().cloned()
                                ).collect::<Result<Vec<_>, _>>()?;
                                let mut shader = Shader::new();
                                parse::parse(&tokens, &mut shader)?;
                                analyse::analyse(&shader)?;
                                let vertex_string = generate::generate(ShaderKind::Vertex, &shader);
                                let fragment_string = generate::generate(ShaderKind::Fragment, &shader);
                                println!("VERTEX SHADER:");
                                println!("{}", vertex_string);
                                println!("FRAGMENT SHADER:");
                                println!("{}", fragment_string);
                                Ok(())
                            })() { 
                                println!("{}", error);
                            }
                            */
                            
                        }
                    }
                },
                _ => ()
            }
        }
    }
}

