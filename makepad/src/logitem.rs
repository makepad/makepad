use render::*;
use editor::*;
use hub::*;

#[derive(Clone)]
pub struct LogItem {
    pub code_editor: CodeEditor,
    pub text_buffer: TextBuffer,
    pub needs_formatting: bool
}

impl LogItem {
    pub fn proto(cx: &mut Cx) -> Self {
        let editor = Self {
            code_editor: CodeEditor {
                draw_line_numbers: false,
                draw_cursor_row: false,
                line_number_width: 10.,
                top_padding: 10.,
                mark_unmatched_parens: false,
                folding_depth: 3,
                ..CodeEditor::proto(cx)
            },
            text_buffer: TextBuffer {
                is_loading: true,
                signal: cx.new_signal(),
                mutation_id: 1,
                ..TextBuffer::default()
            },
            needs_formatting: false,
        };
        editor
    }
    
    pub fn load_loc_message(&mut self, cx: &mut Cx, loc_message: &LocMessage) {
        self.needs_formatting = true;

        let text = if let Some(rendered) = &loc_message.rendered {
            if let Some(explanation) = &loc_message.explanation {
                format!("{}{}{}", loc_message.body, rendered, explanation)
            }
            else {
                format!("{}{}", loc_message.body,rendered)
            }
        }
        else {
            loc_message.body.clone()
        };
        
        self.text_buffer.load_from_utf8(cx, &text);
        self.code_editor.view.redraw_view_area(cx);
    }

    pub fn load_plain_text(&mut self, cx: &mut Cx, val: &str) {
        println!("PLAIN TEXT {}", val);
        self.needs_formatting = false;
        self.text_buffer.load_from_utf8(cx, val);
        self.code_editor.view.redraw_view_area(cx);
    }


    pub fn clear_msg(&mut self, cx: &mut Cx) {
        self.text_buffer.load_from_utf8(cx, "");
    }
    
    pub fn handle_log_item(&mut self, cx: &mut Cx, event: &mut Event) -> CodeEditorEvent {
        let text_buffer = &mut self.text_buffer;
        let ce = self.code_editor.handle_code_editor(cx, event, text_buffer);
        match ce {
            CodeEditorEvent::AutoFormat => {
                let formatted = RustTokenizer::auto_format(text_buffer, true).out_lines;
                self.code_editor.cursors.replace_lines_formatted(formatted, text_buffer);
                self.code_editor.view.redraw_view_area(cx);
            },
            _ => ()
        }
        ce
        
    }
    
    pub fn draw_log_item(&mut self, cx: &mut Cx) {
        let text_buffer = &mut self.text_buffer;
        if text_buffer.needs_token_chunks() && text_buffer.lines.len() >0 {
            
            if self.needs_formatting {
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
            else{
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
        }
        
        if self.code_editor.begin_code_editor(cx, text_buffer).is_err() {return}

        for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate() {
            self.code_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.messages.cursors);
        }
        
        self.code_editor.end_code_editor(cx, text_buffer);
    }
}