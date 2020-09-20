use makepad_render::*;
use crate::texteditor::*;
use crate::textbuffer::*;
use crate::tokentype::*;

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
    
    pub fn style(cx: &mut Cx) {
        
        live!(cx, r#"
            self::color_empty_message: #666;
            
            self::style_text_input: Style {
                
                crate::texteditor::gutter_width: 0.0;
                crate::texteditor::padding_top: 0.0;
                
                crate::texteditor::layout_bg: Layout {
                    walk: {
                        width: Compute,
                        height: Compute,
                        margin: {t: 4., l: 0., r: 0., b: 0.}
                    },
                    padding: all(7.),
                }
                
                crate::texteditor::shader_bg: Shader {
                    use makepad_render::quad::shader::*;
                    fn pixel() -> vec4 {
                        let cx = Df::viewport(pos * vec2(w, h));
                        cx.box(0., 0., w, h, 2.5);
                        return cx.fill(color);
                    }
                }
            }
        "#)
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
    
    pub fn select_all(&mut self, cx: &mut Cx) {
        self.text_editor.cursors.select_all(&mut self.text_buffer);
        self.text_editor.view.redraw_view_area(cx);
    }
    
    pub fn draw_text_input_static(&mut self, cx: &mut Cx, text: &str) {
        let text_buffer = &mut self.text_buffer;
        text_buffer.load_from_utf8(text);
        self.draw_text_input(cx);
    }
    
    pub fn draw_text_input(&mut self, cx: &mut Cx) {
        live_style_begin!(cx, self::style_text_input);
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
        
        if self.text_editor.begin_text_editor(cx, text_buffer).is_err() {
            live_style_end!(cx, self::style_text_input);
            return;
        }
        
        if text_buffer.is_empty() {
            let pos = cx.get_turtle_pos();
            self.text_editor.text.color = live_color!(cx, self::color_empty_message);
            self.text_editor.text.draw_text(cx, &self.empty_message);
            cx.set_turtle_pos(pos);
        }
        
        for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate() {
            self.text_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.markers);
        }
        
        self.text_editor.end_text_editor(cx, text_buffer);
        live_style_end!(cx, self::style_text_input);
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
