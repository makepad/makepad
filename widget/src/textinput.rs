use render::*;
use crate::texteditor::*;
use crate::textbuffer::*;
use crate::plaineditor::*;
use crate::widgetstyle::*;
#[derive(Clone)]
pub struct TextInput {
    pub text_editor: TextEditor,
    pub text_buffer: TextBuffer,
}

pub struct TextInputOptions {
    pub multiline: bool,
    pub read_only: bool,
    pub empty_message: String
}

impl TextInput {
    
    pub fn proto(cx: &mut Cx, opt: TextInputOptions) -> Self {
        Self {
            text_editor: TextEditor {
                read_only: opt.read_only,
                multiline: opt.multiline,
                empty_message: opt.empty_message,
                draw_line_numbers: false,
                draw_cursor_row: false,
                highlight_area_on: false,
                line_number_width: 0.,
                top_padding: 0.,
                mark_unmatched_parens: false,
                view_layout: Layout {
                    walk: Walk::wh(Width::Fix(150.), Height::Compute),
                    padding: Padding::all(10.),
                    ..Layout::default()
                },
                folding_depth: 3,
                ..TextEditor::proto(cx)
            },
            text_buffer: TextBuffer::from_utf8(""),
        }
    }
    
    pub fn text_input_style()->StyleId{uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        cx.begin_style(Self::text_input_style());
        TextEditor::color_bg().set(cx, Theme::color_bg_normal().get(cx));
        TextEditor::gutter_width().set(cx, 0.);
        TextEditor::padding_top().set(cx, 0.);
        cx.end_style(Self::text_input_style());
    }
    
    pub fn handle_plain_text(&mut self, cx: &mut Cx, event: &mut Event) -> TextEditorEvent {
        let text_buffer = &mut self.text_buffer;
        let ce = self.text_editor.handle_text_editor(cx, event, text_buffer);
        ce
    }
    
    pub fn set_value(&mut self, cx: &mut Cx, text: &str) {
        let text_buffer = &mut self.text_buffer;
        text_buffer.load_from_utf8(text);
        self.text_editor.view.redraw_view_area(cx);
    }
    
    pub fn get_value(&self) -> String {
        self.text_buffer.get_as_string()
    }
    
    pub fn draw_plain_text_static(&mut self, cx: &mut Cx, text: &str) {
        let text_buffer = &mut self.text_buffer;
        text_buffer.load_from_utf8(text);
        self.draw_plain_text(cx);
    }
    
    pub fn draw_plain_text(&mut self, cx: &mut Cx) {
        cx.begin_style(Self::text_input_style());
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
        
        if self.text_editor.begin_text_editor(cx, text_buffer).is_err() {return}
        
        for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate() {
            self.text_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.messages.cursors);
        }
        
        self.text_editor.end_text_editor(cx, text_buffer);
        cx.end_style(Self::text_input_style());
    }
}