use crate::rusteditor::*;
use crate::plaineditor::*;
use makepad_render::*;
use makepad_hub::*;
use makepad_widget::*;

#[derive(Clone, PartialEq)]
pub enum ItemDisplayHistory {
    Plain {text: String},
    Message {message: LocMessage},
    Rust {text_buffer_id: TextBufferId, pos: TextPos},
}

#[derive(Clone)]
pub struct ItemDisplay {
    pub history: Vec<ItemDisplayHistory>,
    pub current: usize,
    pub rust_disp: RustEditor,
    pub text_disp: TextEditor,
    pub text_buffer: TextBuffer,
    pub prev_button: NormalButton,
    pub next_button: NormalButton,
    pub open_button: NormalButton,
    pub item_title: Text
}

impl ItemDisplay {
    pub fn proto(cx: &mut Cx) -> Self {
        let editor = Self {
            text_disp: TextEditor {
                read_only: true,
                draw_line_numbers: false,
                draw_cursor_row: false,
                mark_unmatched_parens: false,
                folding_depth: 3,
                ..TextEditor::proto(cx)
            },
            text_buffer: TextBuffer {
                ..TextBuffer::default()
            },
            rust_disp: RustEditor {
                text_editor: TextEditor {
                    read_only: true,
                    ..RustEditor::proto(cx).text_editor
                },
                ..RustEditor::proto(cx)
            },
            prev_button: NormalButton::proto(cx),
            next_button: NormalButton::proto(cx),
            open_button: NormalButton::proto(cx),
            item_title: Text::proto(cx),
            history: Vec::new(),
            current: 0
        };
        editor
    }
    
    pub fn style_text_editor() -> StyleId {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        cx.begin_style(Self::style_text_editor());
        TextEditor::gutter_width().set(cx, 10.);
        TextEditor::padding_top().set(cx, 10.);
        cx.end_style();
    }
    
    pub fn load_message(&mut self, cx: &mut Cx, loc_message: &LocMessage) {

        let text_buffer = &mut self.text_buffer;
        let text = if let Some(rendered) = &loc_message.rendered {
            if let Some(explanation) = &loc_message.explanation {
                format!("{}{}{}", loc_message.body, rendered, explanation)
            }
            else {
                format!("{}{}", loc_message.body, rendered)
            }
        }
        else {
            loc_message.body.clone()
        };
        
        text_buffer.load_from_utf8(&text);
        
        if text_buffer.needs_token_chunks() && text_buffer.lines.len() >0 {
            
            let mut state = TokenizerState::new(&text_buffer.lines);
            let mut tokenizer = RustTokenizer::new();
            let mut pair_stack = Vec::new();
            let mut line_count = 0;
            let mut token_count = 0;
            let mut backtick_toggle = false;
            let mut first_block = false;
            let mut first_block_code_line = false;
            let mut message_type = TokenType::Warning;
            loop {
                let offset = text_buffer.flat_text.len();
                let mut token_type = tokenizer.next_token(&mut state, &mut text_buffer.flat_text, &text_buffer.token_chunks);
                let mut val = String::new();
                for i in offset..text_buffer.flat_text.len() {
                    val.push(text_buffer.flat_text[i]);
                }
                if token_type == TokenType::Operator && val == "`" {
                    backtick_toggle = !backtick_toggle;
                }
                
                let inside_backtick = !backtick_toggle || token_type == TokenType::Operator && val == "`";
                if line_count == 2 {
                    first_block = true;
                }
                if first_block && token_count == 0 && token_type == TokenType::Number {
                    first_block_code_line = true;
                }
                
                // Gray out everything thats not in backticks or code
                if (line_count == 0 && inside_backtick || line_count == 1 || first_block && token_count <= 2 && (val == "|" || token_type == TokenType::Number) || first_block && !first_block_code_line && inside_backtick || !first_block && inside_backtick)
                    && token_type != TokenType::Whitespace
                    && token_type != TokenType::Newline
                    && token_type != TokenType::Eof {
                    token_type = TokenType::Defocus;
                }
                
                // color the ^^
                if first_block && !first_block_code_line && val == "^" {
                    token_type = message_type;
                }
                
                if first_block && token_count == 1 && val != "|" && token_type != TokenType::Whitespace {
                    first_block = false;
                }
                
                if line_count == 0 && token_count == 0 {
                    if val == "warning" {
                        token_type = TokenType::Warning
                    }
                    else if val == "error" {
                        message_type = TokenType::Error;
                        token_type = TokenType::Error
                    }
                }
                
                
                //println!("{:?} {}", token_type, val);
                
                TokenChunk::push_with_pairing(&mut text_buffer.token_chunks, &mut pair_stack, state.next, offset, text_buffer.flat_text.len(), token_type);
                
                token_count += 1;
                if token_type == TokenType::Newline {
                    line_count += 1;
                    token_count = 0;
                    first_block_code_line = false;
                }
                if token_type == TokenType::Eof {
                    break
                }
            }
        }
        
        self.text_disp.view.redraw_view_area(cx);
    }
    
    pub fn load_plain_text(&mut self, cx: &mut Cx, val: &str) {
        let text_buffer = &mut self.text_buffer;

        text_buffer.load_from_utf8(val);
        
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
        
        self.text_disp.view.redraw_view_area(cx);
    }
    
    pub fn load_file(&mut self, _cx: &mut Cx, _text_buffer_id: TextBufferId, _pos: TextPos) {
    }
    
    pub fn handle_item_display(&mut self, cx: &mut Cx, event: &mut Event) {
        if self.current < self.history.len() {
            self.text_disp.handle_text_editor(cx, event, &mut self.text_buffer);
        }
    }
    
    pub fn draw_item_display(&mut self, cx: &mut Cx) {
        
        if self.current < self.history.len() {
            
            let text_buffer = &mut self.text_buffer;
            cx.begin_style(Self::style_text_editor());
            
            if self.text_disp.begin_text_editor(cx, text_buffer).is_err() {return cx.end_style();}
            
            for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate() {
                self.text_disp.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.markers);
            }
            
            self.text_disp.end_text_editor(cx, text_buffer);
            
            cx.end_style();
        }
    }
}
