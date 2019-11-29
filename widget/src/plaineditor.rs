use render::*; 

use crate::textbuffer::*;
use crate::texteditor::*;

#[derive(Clone)]  
pub struct PlainEditor {
    pub text_editor: TextEditor,
}

impl PlainEditor {
    pub fn proto(cx: &mut Cx) -> Self {
        let editor = Self {
            text_editor: TextEditor{
                folding_depth: 3,
                ..TextEditor::proto(cx)
            }
        };
        editor
    }
    
    pub fn handle_plain_editor(&mut self, cx: &mut Cx, event: &mut Event,  text_buffer: &mut TextBuffer) -> TextEditorEvent {
        let ce = self.text_editor.handle_text_editor(cx, event, text_buffer);
        ce
    }
    
    pub fn draw_plain_editor(&mut self, cx: &mut Cx, text_buffer: &mut TextBuffer) {
        if text_buffer.needs_token_chunks() && text_buffer.lines.len() >0{
            let mut state = TokenizerState::new(&text_buffer.lines);
            let mut tokenizer = PlainTokenizer::new();
            let mut pair_stack = Vec::new();
            loop {
                let offset = text_buffer.flat_text.len();
                let token_type = tokenizer.next_token(&mut state, &mut text_buffer.flat_text, &text_buffer.token_chunks);
                TokenChunk::push_with_pairing(&mut text_buffer.token_chunks, &mut pair_stack, state.next, offset, text_buffer.flat_text.len(), token_type);
                if token_type == TokenType::Eof {
                    break
                }
            }
        }
        
        if self.text_editor.begin_text_editor(cx, text_buffer).is_err() {return}
        
        for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate(){
            self.text_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.messages.cursors);
        }
        
        self.text_editor.end_text_editor(cx, text_buffer);
    }
}

pub struct PlainTokenizer {
}

impl PlainTokenizer {
    pub fn new() -> PlainTokenizer {
        PlainTokenizer {}
    }
    
    pub fn next_token<'a>(&mut self, state: &mut TokenizerState<'a>, chunk: &mut Vec<char>, _token_chunks: &Vec<TokenChunk>) -> TokenType {
        let start = chunk.len();
        loop {
            if state.next == '\0' {
		if (chunk.len()-start)>0 { 
                    return TokenType::Identifier
                }
		state.advance();
                chunk.push(' ');
                return TokenType::Eof
            }
            else if state.next == '\n' {
                // output current line
                if (chunk.len()-start)>0 {
                    return TokenType::Identifier
                }
                
                chunk.push(state.next);
                state.advance();
                return TokenType::Newline
            }
            else if state.next == ' ' {
                if (chunk.len()-start)>0 {
                    return TokenType::Identifier
                }
                while state.next == ' ' {
                    chunk.push(state.next);
                    state.advance();
                }
                return TokenType::Whitespace
            }
            else {
                chunk.push(state.next);
                state.advance();
            }
        }
    }
}

