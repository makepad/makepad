use widgets::*;
use crate::textbuffer::*;
use crate::codeeditor::*;

#[derive(Clone)]
pub struct RustEditor{
    pub path:String,
    pub code_editor:CodeEditor,
    pub col_whitespace:Color,
    pub col_keyword:Color,
    pub col_flow_keyword:Color,
    pub col_identifier:Color,
    pub col_operator:Color,
    pub col_function:Color,
    pub col_number:Color,
    pub col_paren:Color,
    pub col_comment:Color,
    pub col_string:Color,
    pub col_delim:Color,
    pub col_type:Color
}

impl ElementLife for RustEditor{
    fn construct(&mut self, _cx:&mut Cx){}
    fn destruct(&mut self, _cx:&mut Cx){}
}

impl Style for RustEditor{
    fn style(cx:&mut Cx)->Self{
        let rust_editor = Self{
            path:"".to_string(),
            code_editor:CodeEditor{
                ..Style::style(cx)
            },
            // syntax highlighting colors
            col_whitespace:color256(110,110,110),
            col_keyword:color256(91,155,211),
            col_flow_keyword:color256(196,133,190),
            col_identifier:color256(212,212,212),
            col_operator:color256(212,212,212),
            col_function:color256(220,220,174),
            col_type:color256(86,201,177),
            col_number:color256(182,206,170),
            col_comment:color256(99,141,84),
            col_paren:color256(212,212,212),
            col_string:color256(204,145,123),
            col_delim:color256(212,212,212)
        };
        //tab.animator.default = tab.anim_default(cx);
        rust_editor
    }
}

#[derive(Clone, PartialEq)]
pub enum RustEditorEvent{
    None,
    Change
}


impl RustEditor{
    pub fn handle_rust_editor(&mut self, cx:&mut Cx, event:&mut Event, text_buffer:&mut TextBuffer)->CodeEditorEvent{
        match self.code_editor.handle_code_editor(cx, event, text_buffer){
            _=>()
        }
        
        CodeEditorEvent::None
    }

    pub fn draw_rust_editor(&mut self, cx:&mut Cx, text_buffer:&TextBuffer){
        if !self.code_editor.begin_code_editor(cx, text_buffer){
            return
        }

        let mut chunk = Vec::new();
        let mut state = TokenizerState::new(text_buffer);
        
        let mut after_newline = true; 
        let mut last_tabs = 0;
        let mut newline_tabs = 0;

        loop{
            //let bit = rust_colorizer.next(&mut chunk, &self.rust_colors);
            let mut do_newline = false;
            let mut is_whitespace = false;
            let mut pop_paren = None;
            let color;
            state.advance_with_cur();
            
            match state.cur{
                '\0'=>{ // eof
                    break;
                },
                '\n'=>{
                    color = self.col_whitespace;
                    // do a newline
                    if after_newline{
                        self.code_editor.draw_tab_lines(cx, last_tabs);
                    }
                    else {
                        last_tabs = newline_tabs;
                    }
                    chunk.push('\n');
                    do_newline = true;
                    after_newline = true;
                    is_whitespace = true;
                    newline_tabs = 0;
                    // spool up the next char
                },
                ' ' | '\t'=>{ // eat as many spaces as possible
                    color = self.col_whitespace;
                    if after_newline{ // consume spaces in groups of 4
                        chunk.push(state.cur);
                        
                        let mut counter = 1;
                        while state.next == ' ' || state.next == '\t'{
                            chunk.push(state.next);
                            counter += 1;
                            state.advance();
                        }
                        let tabs = counter >> 2;
                        last_tabs = tabs;
                        newline_tabs = tabs;
                        self.code_editor.draw_tab_lines(cx, tabs);
                    }
                    else{
                        chunk.push(state.cur);
                        while state.next == ' '{
                            chunk.push(state.next);
                            state.advance();
                        }
                    }
                    is_whitespace = true;
                },
                '/'=>{ // parse comment
                    after_newline = false;
                    chunk.push(state.cur);
                    if state.next == '/'{
                        while state.next != '\n' && state.next != '\0'{
                            chunk.push(state.next);
                            state.advance();
                        }
                        color = self.col_comment;
                    }
                    else{
                        if state.next == '='{
                            chunk.push(state.next);
                            state.advance();
                        }
                        color = self.col_operator;
                    }
                },
                '\''=>{ // parse char literal or lifetime annotation

                    after_newline = false;
                    chunk.push(state.cur);

                    if Self::parse_rust_escape_char(&mut state, &mut chunk){ // escape char or unicode
                        if state.next == '\''{ // parsed to closing '
                            chunk.push(state.next);
                            state.advance();
                            color = self.col_string;
                        }
                        else{
                            color = self.col_comment;
                        }
                    }
                    else{ // parse a single char or lifetime
                        let offset = state.offset;
                        if Self::parse_rust_ident_tail(&mut state, &mut chunk) && ((state.offset - offset) > 1 || state.next != '\''){
                            color = self.col_keyword;
                        }
                        else if state.next != '\n'{
                            if (state.offset - offset) == 0{ // not an identifier char
                                chunk.push(state.next);
                                state.advance();
                            }
                            if state.next == '\''{ // lifetime identifier
                                chunk.push(state.next);
                                state.advance();
                            }
                            color = self.col_string;
                        }
                        else{
                            color = self.col_string;                            
                        }
                    }
                },
                '"'=>{ // parse string
                    after_newline = false;
                    chunk.push(state.cur);
                    state.prev = '\0';
                    while state.next != '\0' && state.next!='\n' && (state.next != '"' || state.prev != '\\' && state.cur == '\\' && state.next == '"'){
                        chunk.push(state.next);
                        state.advance_with_prev();
                    };
                    chunk.push(state.next);
                    state.advance();
                    color = self.col_string;
                },
                '0'...'9'=>{ // try to parse numbers
                    after_newline = false;
                    color = self.col_number;
                    chunk.push(state.cur);
                    Self::parse_rust_number_tail(&mut state, &mut chunk);
                },
                ':'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    if state.next == ':'{
                        chunk.push(state.next);
                        state.advance();
                    }
                    color = self.col_operator;
                },
                '*'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    if state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }                    
                    color = self.col_operator;
                },
                '+'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    if state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    color = self.col_operator;
                },
                '-'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    if state.next == '>' || state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    color = self.col_operator;
                },
                '='=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    if state.next == '>' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    color = self.col_operator;
                },
                '.'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    if state.next == '.' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    color = self.col_operator;
                },
                '('=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    color = self.col_paren;
                    self.code_editor.push_paren_stack(cx, ParenType::Round);
                },
                ')'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    color = self.col_paren;
                    pop_paren = Some(ParenType::Round);
                },
                '{'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    color = self.col_paren;
                    self.code_editor.push_paren_stack(cx, ParenType::Curly);
                },
                '}'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    color = self.col_paren;
                    pop_paren = Some(ParenType::Curly);
                },
                '['=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    color = self.col_paren;
                    self.code_editor.push_paren_stack(cx, ParenType::Square);
                },
                ']'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    color = self.col_paren;
                    pop_paren = Some(ParenType::Square);
                },
                '_'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    Self::parse_rust_ident_tail(&mut state, &mut chunk);
                    color = self.col_identifier;
                },
                'a'...'z'=>{ // try to parse keywords or identifiers
                    after_newline = false;
                    chunk.push(state.cur);
                    let mut keyword_type = Self::parse_rust_lc_keyword(&mut state, &mut chunk);

                    if Self::parse_rust_ident_tail(&mut state, &mut chunk){
                        keyword_type = KeywordType::None;
                    }
                    match keyword_type{
                        KeywordType::Normal=>{
                            color = self.col_keyword;
                        },
                        KeywordType::Flow=>{
                            color = self.col_flow_keyword;
                        },
                        KeywordType::None=>{
                            if state.next == '(' || state.next == '!'{
                                color = self.col_function;
                            }
                            else{
                                color = self.col_identifier;
                            }
                        }
                    }
                },
                'A'...'Z'=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    let mut is_keyword = false;
                    if state.cur == 'S'{
                        if state.keyword(&mut chunk, "elf"){
                            is_keyword = true;
                        }
                    }
                    if Self::parse_rust_ident_tail(&mut state, &mut chunk){
                        is_keyword = false;
                    }
                    if is_keyword{
                        color = self.col_keyword;
                    }
                    else{
                        color = self.col_type;
                    }
                },
                _=>{
                    after_newline = false;
                    chunk.push(state.cur);
                    // unknown type
                    color = self.col_identifier;
                }
            }
            self.code_editor.draw_text(cx, &chunk, state.offset, is_whitespace, color);
            chunk.truncate(0);
            if let Some(paren_type) = pop_paren{
                self.code_editor.pop_paren_stack(cx, paren_type);
            }
            if do_newline{
                self.code_editor.new_line(cx);
            }
        }
        
        self.code_editor.end_code_editor(cx, text_buffer);
    }

    fn parse_rust_ident_tail<'a>(state:&mut TokenizerState<'a>, chunk:&mut Vec<char>)->bool{
        let mut ret = false;
        while state.next_is_digit() || state.next_is_letter() || state.next == '_' || state.next == '$'{
            ret = true;
            chunk.push(state.next);
            state.advance();
        }
        ret
    }

    fn parse_rust_escape_char<'a>(state:&mut TokenizerState<'a>, chunk:&mut Vec<char>)->bool{
        if state.next == '\\'{
            chunk.push(state.next);
            state.advance();
            if state.next == 'u'{
                chunk.push(state.next);
                state.advance();
                if state.next == '{'{
                    chunk.push(state.next);
                    state.advance();
                    while state.next_is_hex(){
                        chunk.push(state.next);
                        state.advance();
                    }
                    if state.next == '}'{
                        chunk.push(state.next);
                        state.advance();
                    }
                }
            }
            else{
                // its a single char escape TODO limit this to valid escape chars
                chunk.push(state.next);
                state.advance();
            }
            return true
        }
        return false
    }

    fn parse_rust_number_tail<'a>(state:&mut TokenizerState<'a>, chunk:&mut Vec<char>){
        if state.next == 'x'{ // parse a hex number
            chunk.push(state.next);
            state.advance();
            while state.next_is_hex() || state.next == '_'{
                chunk.push(state.next);
                state.advance();
            }
        }
        else if state.next == 'b'{ // parse a binary
            chunk.push(state.next);
            state.advance();
            while state.next == '0' || state.next =='1' || state.next == '_'{
                chunk.push(state.next);
                state.advance();
            }
        }
        else{
            while state.next_is_digit() || state.next == '_'{
                chunk.push(state.next);
                state.advance();
            }
            if state.next == 'u' || state.next == 'i'{
                chunk.push(state.next);
                state.advance();
                if state.keyword(chunk, "8"){
                }
                else if state.keyword(chunk, "16"){
                }
                else if state.keyword(chunk,"32"){
                }
                else if state.keyword(chunk,"64"){
                }
            }
            else if state.next == '.'{
                chunk.push(state.next);
                state.advance();
                // again eat as many numbers as possible
                while state.next_is_digit() || state.next == '_'{
                    chunk.push(state.next);
                    state.advance();
                }
                if state.next == 'f' { // the f32, f64 postfix
                    chunk.push(state.next);
                    state.advance();
                    if state.keyword(chunk,"32"){
                    }
                    else if state.keyword(chunk,"64"){
                    }
                }
            }
        }
    }

    fn parse_rust_lc_keyword<'a>(state:&mut TokenizerState<'a>, chunk:&mut Vec<char>)->KeywordType{
        match state.cur{
            'a'=>{
                if state.keyword(chunk,"s"){
                    return KeywordType::Normal
                }
            },
            'b'=>{ 
                if state.keyword(chunk,"reak"){
                    return KeywordType::Flow
                }
            },
            'c'=>{
                if state.keyword(chunk,"o"){
                    if state.keyword(chunk,"nst"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk,"ntinue"){
                        return KeywordType::Flow
                    }
                }
                else if state.keyword(chunk,"rate"){
                    return KeywordType::Normal
                }
            },
            'e'=>{
                if state.keyword(chunk,"lse"){
                    return KeywordType::Flow
                }
                else if state.keyword(chunk,"num"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk,"xtern"){
                    return KeywordType::Normal
                }
            },
            'f'=>{
                if state.keyword(chunk,"alse"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk,"n"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk,"or"){
                    return KeywordType::Flow
                }
            },
            'i'=>{
                if state.keyword(chunk,"f"){
                    return KeywordType::Flow
                }
                else if state.keyword(chunk,"mpl"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk,"in"){
                    return KeywordType::Normal
                }
            },
            'l'=>{
                if state.keyword(chunk,"et"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk,"oop"){
                    return KeywordType::Flow
                }
            },
            'm'=>{
                if state.keyword(chunk,"atc"){
                    return KeywordType::Flow
                }
                else if state.keyword(chunk,"o"){
                    if state.keyword(chunk,"d"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk,"ve"){
                        return KeywordType::Normal
                    }
                }
                else if state.keyword(chunk,"ut"){
                    return KeywordType::Normal
                }
            },
            'p'=>{ // pub
                if state.keyword(chunk,"ub"){ 
                    return KeywordType::Normal
                }
            },
            'r'=>{
                if state.keyword(chunk,"e"){
                    if state.keyword(chunk,"f"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk,"turn"){
                        return KeywordType::Flow
                    }
                }
            },
            's'=>{
                if state.keyword(chunk,"elf"){
                    return KeywordType::Normal
                }
                if state.keyword(chunk,"uper"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk,"t"){
                    if state.keyword(chunk,"atic"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk,"ruct"){
                        return KeywordType::Normal
                    }
                }
            },
            't'=>{
                if state.keyword(chunk,"ype"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk,"r"){
                    if state.keyword(chunk,"rait"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk,"ue"){
                        return KeywordType::Normal
                    }
                }
            },
            'u'=>{ // use
                if state.keyword(chunk,"se"){ 
                    return KeywordType::Normal
                }
                else if state.keyword(chunk,"nsafe"){ 
                    return KeywordType::Normal
                }
            },
            'w'=>{ // use
                if state.keyword(chunk,"h"){
                    if state.keyword(chunk,"ere"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk,"ile"){
                        return KeywordType::Flow
                    }
                }
            }, 
            _=>{}
        }     
        KeywordType::None
    }
}

enum KeywordType{
    None,
    Normal,
    Flow,
}