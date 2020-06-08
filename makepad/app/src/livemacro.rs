
use makepad_render::*;
use makepad_widget::*;
use crate::mprstokenizer::*;
pub struct LiveMacroPick{
    token:usize,
}

pub struct LiveMacroShader{
    token:usize,
}

pub enum LiveMacro {
    Pick(LiveMacroPick),
    Shader(LiveMacroShader)
}

#[derive(Default)]
pub struct LiveMacros{
    macros: Vec<LiveMacro>
}

impl LiveMacros{
    pub fn parse(&mut self, text_buffer:&TextBuffer){
        let mut tp = TokenParser::new(&text_buffer.flat_text, &text_buffer.token_chunks);
        while tp.advance() {
            match tp.cur_type(){
                TokenType::Macro=>{
                    if tp.eat("shader") &&
                        tp.eat("!") && 
                        tp.eat("{") {
                            // ok now we are at ", hopefully
                            // get matching pair
                            if tp.cur_type() == TokenType::ParenOpen{
                                let pair_token = tp.cur_pair_token();
                                // jump to end.
                                
                            } 
                            // now we should have a {
                            // and then a "
                            // and we should run to matching }
                            // 
                            // lets convert our mprs tokens to
                            // shader tokens now
                            // send in a TokenParser
                            // 
                        }
                },
                _=>()
            }
        }
        //let new_tok = &text_buffer.token_chunks[new_index];
        //let new_tok_slice = &text_buffer.flat_text[new_tok.offset..new_tok.offset + new_tok.len];
        //let old_tok = &text_buffer.old_token_chunks[old_index];
        //let old_tok_slice = &text_buffer.flat_text[old_tok.offset..old_tok.offset + old_tok.len];
    }
}