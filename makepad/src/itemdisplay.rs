use crate::rusteditor::*;
use crate::plaineditor::*;
use crate::appstorage::*;
use makepad_render::*;
use makepad_hub::*;
use makepad_widget::*;

#[derive(Clone, PartialEq)]
pub enum ItemDisplayHistory {
    PlainText {text: String},
    Message {message: LocMessage},
    Rust {text_buffer_id: TextBufferId, offset: usize},
}

#[derive(Clone)]
pub struct ItemDisplay {
    pub history: Vec<ItemDisplayHistory>,
    pub current: usize,
    pub update_display: bool,
    pub view: View,
    pub rust_disp: RustEditor,
    pub text_disp: TextEditor,
    pub text_buffer: TextBuffer,
    pub last_text_buffer_id: usize,
    pub prev_button: NormalButton,
    pub next_button: NormalButton,
    pub open_button: NormalButton,
    pub item_title: Text
}

impl ItemDisplay {
    pub fn proto(cx: &mut Cx) -> Self {
        let editor = Self {
            view: View::proto(cx),
            update_display: false,
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
                    view:ScrollView{
                        scroll_v:Some(ScrollBar::proto(cx)),
                        ..ScrollView::proto(cx)
                    },
                    read_only: true,
                    jump_to_offset_at_top: true,
                    ..RustEditor::proto(cx).text_editor
                },
                ..RustEditor::proto(cx)
            },
            last_text_buffer_id: 65537,
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
    pub fn style_rust_editor() -> StyleId {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        cx.begin_style(Self::style_text_editor());
        TextEditor::gutter_width().set(cx, 10.);
        TextEditor::padding_top().set(cx, 10.);
        TextEditor::color_bg().set(cx, Theme::color_bg_odd().get(cx));
        cx.end_style();
        cx.begin_style(Self::style_rust_editor());
        TextEditor::color_bg().set(cx, Theme::color_bg_odd().get(cx));
        TextEditor::color_gutter_bg().set(cx, Theme::color_bg_odd().get(cx));
        cx.end_style();
    }
    
    pub fn display_message(&mut self, cx: &mut Cx, loc_message: &LocMessage) {
        self.history.truncate(self.current);
        self.history.push(
            ItemDisplayHistory::Message {message: loc_message.clone()}
        );
        self.current = self.history.len() - 1;
        self.update_display = true;
        self.view.redraw_view_parent_area(cx);
    }
    
    pub fn display_plain_text(&mut self, cx: &mut Cx, val: &str) {
        self.history.truncate(self.current);
        self.history.push(
            ItemDisplayHistory::PlainText {text: val.to_string()}
        );
        self.current = self.history.len() - 1;
        self.update_display = true;
        self.view.redraw_view_parent_area(cx);
    }
    
    pub fn display_rust_file(&mut self, cx: &mut Cx, text_buffer_id: TextBufferId, offset: usize) {
        self.history.truncate(self.current);
        self.history.push(
            ItemDisplayHistory::Rust {text_buffer_id, offset}
        );
        self.current = self.history.len() - 1;
        self.update_display = true;
        self.view.redraw_view_parent_area(cx);
    }
    
    pub fn update_plain_text_buffer(text_buffer: &mut TextBuffer, text: &str) {
        text_buffer.load_from_utf8(text);
        PlainTokenizer::update_token_chunks(text_buffer, None);
    }
    
    pub fn update_message_text_buffer(text_buffer: &mut TextBuffer, loc_message: &LocMessage) {
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
    }
    
    pub fn handle_item_display(&mut self, cx: &mut Cx, event: &mut Event, storage:&mut AppStorage)->TextEditorEvent{
        if self.current < self.history.len() {
            match &self.history[self.current] {
                ItemDisplayHistory::PlainText {..} => {
                    self.text_disp.handle_text_editor(cx, event, &mut self.text_buffer)
                },
                ItemDisplayHistory::Message {..} => {
                    self.text_disp.handle_text_editor(cx, event, &mut self.text_buffer)
                },
                ItemDisplayHistory::Rust {text_buffer_id, ..} => {
                    let text_buffer = &mut storage.text_buffers[text_buffer_id.as_index()].text_buffer;
                    self.rust_disp.handle_rust_editor(cx, event, text_buffer)
                }
            }
        }
        else{
            TextEditorEvent::None
        }
    }
    
    pub fn draw_item_display(&mut self, cx: &mut Cx, storage:&mut AppStorage) {
        if self.current < self.history.len() {
            if self.update_display {
                self.update_display = false;
                match &self.history[self.current] {
                    ItemDisplayHistory::PlainText {text} => {
                        Self::update_plain_text_buffer(&mut self.text_buffer, text);
                    },
                    ItemDisplayHistory::Message {message} => {
                        Self::update_message_text_buffer(&mut self.text_buffer, message);
                    },
                    ItemDisplayHistory::Rust {offset,text_buffer_id} => {
                        if self.last_text_buffer_id != text_buffer_id.as_index(){
                            self.last_text_buffer_id = text_buffer_id.as_index();
                            self.rust_disp.text_editor.reset_cursors();
                        }
                        self.rust_disp.text_editor.jump_to_offset(cx, *offset);
                    }
                }
            }
            match self.history[self.current] {
                ItemDisplayHistory::PlainText {..} | ItemDisplayHistory::Message {..} => {
                    let text_buffer = &mut self.text_buffer;
                    cx.begin_style(Self::style_text_editor());
                    if self.text_disp.begin_text_editor(cx, text_buffer).is_err() {return cx.end_style();}
                    
                    for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate() {
                        self.text_disp.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.markers);
                    }
                    self.text_disp.end_text_editor(cx, text_buffer);
                    cx.end_style();
                },
                ItemDisplayHistory::Rust {text_buffer_id, ..} => {
                    cx.begin_style(Self::style_rust_editor());
                    let text_buffer = &mut storage.text_buffers[text_buffer_id.as_index()].text_buffer;
                    self.rust_disp.draw_rust_editor(cx, text_buffer, None);
                    cx.end_style();
                }
            }
        }
    }
}
