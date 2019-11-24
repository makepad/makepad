use render::*;
use crate::codeeditor::*;
use crate::textbuffer::*;
use crate::plaineditor::*;

#[derive(Clone)]
pub struct TextInput {
    pub code_editor: CodeEditor,
    pub text_buffer: TextBuffer,
    pub read_only: bool,
    pub class: ClassId,
}

impl TextInput {
    pub fn proto(cx: &mut Cx) -> Self {
        let mut text_buffer = TextBuffer {
            is_loading: true,
            mutation_id: 1,
            ..TextBuffer::default()
        };
        text_buffer.load_from_utf8(cx, "textinput");
        Self {
            class:ClassId::base(),
            read_only: false,
            code_editor: CodeEditor {
                draw_line_numbers: false,
                draw_cursor_row: false,
                line_number_width: 0.,
                top_padding: 0.,
                mark_unmatched_parens: false,
                bg_layout: Layout{
                    walk:Walk::wh(Width::Compute, Height::Compute),
                    ..Layout::default()
                },
                folding_depth: 3,
                ..CodeEditor::proto(cx)
            },
            text_buffer: text_buffer, 
        }
    }
    
    pub fn handle_plain_text(&mut self, cx: &mut Cx, event: &mut Event) -> CodeEditorEvent {
        let text_buffer = &mut self.text_buffer;
        self.code_editor.read_only = self.read_only;
        let ce = self.code_editor.handle_code_editor(cx, event, text_buffer);
        ce
    }

    pub fn set_value(&mut self, cx: &mut Cx, text:&str) {
        let text_buffer = &mut self.text_buffer;
        text_buffer.load_from_utf8(cx, text);
        self.code_editor.view.redraw_view_area(cx);
    }
    
    pub fn get_value(&self)->String{
        self.text_buffer.get_as_string()        
    }
    
    pub fn draw_plain_text_static(&mut self, cx: &mut Cx, text:&str) {
        let text_buffer = &mut self.text_buffer;
        text_buffer.load_from_utf8(cx, text);
        self.draw_plain_text(cx);
    }
    
    pub fn draw_plain_text(&mut self, cx: &mut Cx) {
        self.code_editor.class = self.class;
        self.code_editor.read_only = self.read_only;
        let text_buffer = &mut self.text_buffer;
        if text_buffer.needs_token_chunks() && text_buffer.lines.len() >0 {
            
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
        
        if self.code_editor.begin_code_editor(cx, text_buffer).is_err() {return}
        
        for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate() {
            self.code_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.messages.cursors);
        }
        
        self.code_editor.end_code_editor(cx, text_buffer);
    }
}