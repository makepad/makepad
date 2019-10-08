use render::*; 
use editor::*;

#[derive(Clone)]
pub struct CargoLogItem {
    pub code_editor: CodeEditor,
    pub text_buffer: TextBuffer
}

impl CargoLogItem {
    pub fn style(cx: &mut Cx) -> Self {
        let editor = Self {
            code_editor: CodeEditor{
                folding_depth: 3,
                ..CodeEditor::style(cx)
            },
            text_buffer: TextBuffer::default()
            
        };
        editor
    }
    
    pub fn handle_cargo_log_item(&mut self, cx: &mut Cx, event: &mut Event) -> CodeEditorEvent {
        let ce = self.code_editor.handle_code_editor(cx, event, &mut self.text_buffer);
        ce
    }
    
    pub fn draw_cargo_log_item(&mut self, _cx: &mut Cx) {
        /*
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
        
        if let Err(_) = self.code_editor.begin_code_editor(cx, text_buffer) {
            return
        }
        
        for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate(){
            self.code_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.messages.cursors);
        }
        self.code_editor.end_code_editor(cx, text_buffer);
        */
    }
}
