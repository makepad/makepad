use makepad_render::*;
use makepad_widget::*;
use crate::searchindex::*;
use crate::appstorage::*;
use crate::mprstokenizer::*;

#[derive(Clone)]
pub struct RustEditor {
    pub text_editor: TextEditor,
}

impl RustEditor {
    pub fn new(cx: &mut Cx) -> Self {
        let editor = Self {
            text_editor: TextEditor::new(cx),
        };
        //tab.animator.default = tab.anim_default(cx);
        editor
    }
    
    pub fn handle_rust_editor(&mut self, cx: &mut Cx, event: &mut Event, atb: &mut AppTextBuffer, search_index: Option<&mut SearchIndex>) -> TextEditorEvent {
        let ce = self.text_editor.handle_text_editor(cx, event, &mut atb.text_buffer);
        match ce {
            TextEditorEvent::Change => {
                Self::update_token_chunks(atb, search_index);
            },
            TextEditorEvent::AutoFormat => {
                let formatted = MprsTokenizer::auto_format(&atb.text_buffer.flat_text, &atb.text_buffer.token_chunks, false).out_lines;
                self.text_editor.cursors.replace_lines_formatted(formatted, &mut atb.text_buffer);
                self.text_editor.view.redraw_view_area(cx);
            },
            _ => ()
        }
        ce
    }
    
    pub fn draw_rust_editor(&mut self, cx: &mut Cx, atb: &mut AppTextBuffer, search_index: Option<&mut SearchIndex>) {
        Self::update_token_chunks(atb, search_index);
        
        if self.text_editor.begin_text_editor(cx, &mut atb.text_buffer).is_err() {return}
        
        for (index, token_chunk) in atb.text_buffer.token_chunks.iter_mut().enumerate() {
            self.text_editor.draw_chunk(cx, index, &atb.text_buffer.flat_text, token_chunk, &atb.text_buffer.markers);
        }
        
        self.text_editor.end_text_editor(cx, &mut atb.text_buffer);
    }

    pub fn update_token_chunks(atb: &mut AppTextBuffer, mut search_index: Option<&mut SearchIndex>) {
        

        if atb.text_buffer.needs_token_chunks() && atb.text_buffer.lines.len() >0 {
            let mut state = TokenizerState::new(&atb.text_buffer.lines);
            let mut tokenizer = MprsTokenizer::new();
            let mut pair_stack = Vec::new();
            loop {
                let offset = atb.text_buffer.flat_text.len();
                let token_type = tokenizer.next_token(&mut state, &mut atb.text_buffer.flat_text, &atb.text_buffer.token_chunks);
                if TokenChunk::push_with_pairing(&mut atb.text_buffer.token_chunks, &mut pair_stack, state.next, offset, atb.text_buffer.flat_text.len(), token_type) {
                    atb.text_buffer.was_invalid_pair = true;
                }
                
                if token_type == TokenType::Eof {
                    break
                }
                if let Some(search_index) = search_index.as_mut() {
                    search_index.new_rust_token(&atb);
                }
            }
            if pair_stack.len() > 0 {
                atb.text_buffer.was_invalid_pair = true;
            }
            
            // lets parse and generate our live macro set
            atb.live_macros.parse(&atb.text_buffer);
            
            // ok now lets write a diff with the previous one
            /*
            let mut new_index = 0;
            let mut old_index = 0;
            let mut recompile = false;
            let mut macro_index = 0;
            loop {
                if let TokenType::Macro = new_tok.token_type {
                    if tok_cmp("pick", new_tok_slice) {
                        // lets parse the new one
                    }
                    // jump new and old to the end of the macro so diffing can continue
                }
                
                if new_index < atb.text_buffer.token_chunks.len() {
                    new_index += 1;
                }
                else {
                    break
                }
                if old_index + 1 < atb.text_buffer.old_token_chunks.len() {
                    old_index += 1;
                    // lets compare the token at this point
                    let new_tok = &atb.text_buffer.token_chunks[new_index];
                    let new_tok_slice = &atb.text_buffer.flat_text[new_tok.offset..new_tok.offset + new_tok.len];
                    let old_tok = &atb.text_buffer.old_token_chunks[old_index];
                    let old_tok_slice = &atb.text_buffer.flat_text[old_tok.offset..old_tok.offset + old_tok.len];
                    if new_tok_slice != old_tok_slice { // things are different and require a recompile
                        recompile = true;
                    }
                }
                else {
                    recompile = true;
                }
            }*/
        }
    }
}
