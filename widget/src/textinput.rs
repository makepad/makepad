use makepad_render::*;
use crate::texteditor::*;
use crate::textbuffer::*;
use crate::widgetstyle::*;
#[derive(Clone)]
pub struct TextInput {
    pub text_editor: TextEditor,
    pub text_buffer: TextBuffer,
    pub empty_message: String,
}

#[derive(Default)]
pub struct TextInputOptions {
    pub multiline: bool,
    pub read_only: bool,
    pub empty_message: String
}

impl TextInput {
    
    pub fn new(cx: &mut Cx, opt: TextInputOptions) -> Self {
        Self {
            text_editor: TextEditor {
                read_only: opt.read_only,
                multiline: opt.multiline,
                draw_line_numbers: false,
                draw_cursor_row: false,
                highlight_area_on: false,
                mark_unmatched_parens: false,
                folding_depth: 3,
                ..TextEditor::new(cx)
            },
            empty_message: opt.empty_message,
            text_buffer: TextBuffer::from_utf8(""),
        }
    }
    
    pub fn style_text_input() -> StyleId {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        cx.begin_style(Self::style_text_input());
        TextEditor::layout_bg().set(cx, Layout {
            walk: Walk {width: Width::Compute, height: Height::Compute, margin: Margin {t: 4., l: 0., r: 0., b: 0.}},
            padding: Padding::all(7.),
            ..Layout::default()
        });
        TextEditor::color_bg().set(cx, TextEditor::color_bg().get(cx));
        TextEditor::gutter_width().set(cx, 0.);
        TextEditor::padding_top().set(cx, 0.);
        TextEditor::shader_bg().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, 2.5);
                return df_fill(color);
            }
        })));
        cx.end_style();
    }
    
    pub fn handle_text_input(&mut self, cx: &mut Cx, event: &mut Event) -> TextEditorEvent {
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
    
    pub fn select_all(&mut self, cx: &mut Cx){
        self.text_editor.cursors.select_all(&mut self.text_buffer);
        self.text_editor.view.redraw_view_area(cx);
    }
    
    pub fn draw_text_input_static(&mut self, cx: &mut Cx, text: &str) {
        let text_buffer = &mut self.text_buffer;
        text_buffer.load_from_utf8(text);
        self.draw_text_input(cx);
    }
    
    pub fn draw_text_input(&mut self, cx: &mut Cx) {
        cx.begin_style(Self::style_text_input());
        let text_buffer = &mut self.text_buffer;
        if text_buffer.needs_token_chunks() && text_buffer.lines.len() >0 {
            
            let mut state = TokenizerState::new(&text_buffer.lines);
            let mut tokenizer = TextInputTokenizer::new();
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
        
        if self.text_editor.begin_text_editor(cx, text_buffer).is_err() {return cx.end_style();}
        
        if text_buffer.is_empty() {
            let pos = cx.get_turtle_pos();
            self.text_editor.text.color = color("#666");
            self.text_editor.text.draw_text(cx, &self.empty_message);
            cx.set_turtle_pos(pos);
        }
        
        for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate() {
            self.text_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.markers);
        }
        
        self.text_editor.end_text_editor(cx, text_buffer);
        cx.end_style();
    }
}


pub struct TextInputTokenizer {
}

impl TextInputTokenizer {
    pub fn new() -> TextInputTokenizer {
        TextInputTokenizer {}
    }
    
    pub fn next_token<'a>(&mut self, state: &mut TokenizerState<'a>, chunk: &mut Vec<char>, _token_chunks: &Vec<TokenChunk>) -> TokenType {
        let start = chunk.len();
        loop {
            if state.next == '\0' {
                if (chunk.len() - start)>0 {
                    return TokenType::Identifier
                }
                state.advance();
                chunk.push(' ');
                return TokenType::Eof
            }
            else if state.next == '\n' {
                // output current line
                if (chunk.len() - start)>0 {
                    return TokenType::Identifier
                }
                
                chunk.push(state.next);
                state.advance();
                return TokenType::Newline
            }
            else if state.next == ' ' {
                if (chunk.len() - start)>0 {
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
