use widget::*;
use crate::textbuffer::*;
use crate::textcursor::*;
use crate::codeeditor::*;

#[derive(Clone)]
pub struct RustEditor{
    pub path:String,
    pub set_key_focus_on_draw:bool,
    pub code_editor:CodeEditor,
}

impl ElementLife for RustEditor{
    fn construct(&mut self, _cx:&mut Cx){}
    fn destruct(&mut self, _cx:&mut Cx){}
}

impl Style for RustEditor{
    fn style(cx:&mut Cx) -> Self{
        let rust_editor = Self{
            set_key_focus_on_draw:false,
            path:"".to_string(),
            code_editor:CodeEditor{
                ..Style::style(cx)
            },
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
    pub fn handle_rust_editor(&mut self, cx:&mut Cx, event:&mut Event, text_buffer:&mut TextBuffer) -> CodeEditorEvent{
        let ce = self.code_editor.handle_code_editor(cx, event, text_buffer);
        match ce{
            CodeEditorEvent::AutoFormat => {
                self.auto_format(text_buffer);
                self.code_editor.view.redraw_view_area(cx);
            },
            _ => ()
        }
        ce
    }
    
    // because rustfmt is such an insane shitpile to compile or use as a library, here is a stupid version.
    pub fn auto_format(&mut self, text_buffer:&mut TextBuffer){
        let mut state = TokenizerState::new(text_buffer);
        let mut rust_tok = RustTokenizer::new();
        let mut out_lines:Vec<Vec<char>> = Vec::new();
        out_lines.push(Vec::new());
        let mut chunk:Vec<char> = Vec::new();
        let mut first_on_line = true;
        let mut horrible_angle_count = 0; // this is just absolutely NOT nice, but it works somewhat.
        let mut first_after_open = false;
        struct ParenStack{
            expecting_newlines:bool,
            expected_indent:usize,
            angle_counter:usize
        }
        let mut paren_stack:Vec<ParenStack> = Vec::new();
        paren_stack.push(ParenStack{expecting_newlines:true,expected_indent:0,angle_counter:0});
        
        let mut expected_indent = 0;
        
        fn output_indent(out_lines:&mut Vec<Vec<char>>, indent_depth:usize){
            let last_line = out_lines.last_mut().unwrap();
            for _ in 0..indent_depth{
                last_line.push(' ');
            }
        }
        let mut last_token = TokenType::Unexpected;
        loop{
            let token_type = rust_tok.next_token(&mut state, &mut chunk, &self.code_editor.token_chunks);
            match token_type{
                TokenType::Whitespace => {
                    if !first_on_line && state.next != '\n' && last_token != TokenType::ParenOpen{
                        out_lines.last_mut().unwrap().push(' ');
                    }
                },
                TokenType::Newline => {
                   // paren_stack.last_mut().unwrap().angle_counter = 0;
                    if first_on_line{
                        output_indent(&mut out_lines, expected_indent);
                    }
                    
                    if first_after_open{
                        paren_stack.last_mut().unwrap().expecting_newlines = true;
                        expected_indent += 4;
                    }
                    if paren_stack.last_mut().unwrap().expecting_newlines == false{ // we are NOT expecting newlines!
                        
                    }
                    else{
                        first_after_open = false;
                        out_lines.push(Vec::new());
                        first_on_line = true;
                    }
                },
                TokenType::Eof => {break},
                TokenType::ParenOpen => {
                    if first_on_line{
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    
                    paren_stack.push(ParenStack{expecting_newlines:false, expected_indent:expected_indent, angle_counter:0});
                    first_after_open = true;
                    out_lines.last_mut().unwrap().append(&mut chunk);
                },
                TokenType::ParenClose => {
                    let last_line = out_lines.last_mut().unwrap();
                    if last_line.len()>0 && *last_line.last().unwrap() == ' '{
                        last_line.pop();
                    }
                    if last_line.len()>0 && *last_line.last().unwrap() == ','{
                        last_line.pop();
                    }
                    
                    first_after_open = false;
                    if !first_on_line && paren_stack.last().unwrap().expecting_newlines == true{ // we are expecting newlines!
                        out_lines.push(Vec::new());
                        first_on_line = true;
                    }
                    expected_indent = if paren_stack.len()>1{
                        paren_stack.pop().unwrap().expected_indent
                    }
                    else{
                        0
                    };
                    if first_on_line{
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    out_lines.last_mut().unwrap().append(&mut chunk);
                },
                TokenType::CommentLine => {
                    if first_on_line{
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    else{
                        let last_line = out_lines.last_mut().unwrap();
                        last_line.push(' ');
                    }
                    out_lines.last_mut().unwrap().append(&mut chunk);
                },
                TokenType::CommentMultiBegin => {
                    if first_on_line{
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    else{
                        let last_line = out_lines.last_mut().unwrap();
                        last_line.push(' ');
                    }
                    out_lines.last_mut().unwrap().append(&mut chunk);
                },
                TokenType::CommentChunk => {
                    out_lines.last_mut().unwrap().append(&mut chunk);
                },
                TokenType::CommentMultiEnd => {
                    let last_line = out_lines.last_mut().unwrap();
                    last_line.append(&mut chunk);
                    last_line.push(' ');
                },
                TokenType::Delimiter => {
                    if first_on_line{
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    else{
                        let last_line = out_lines.last_mut().unwrap();
                        if last_line.len()>0 && *last_line.last().unwrap() == ' '{
                            last_line.pop();
                        }
                    }
                    let ch = chunk[0];
                    out_lines.last_mut().unwrap().append(&mut chunk);
                    if (ch !=',' || paren_stack.last_mut().unwrap().angle_counter == 0) && 
                    paren_stack.last().unwrap().expecting_newlines == true && state.next != '\n' { // we are expecting newlines!
                        out_lines.push(Vec::new());
                        first_on_line = true;
                    }
                    else if state.next != ' ' && state.next != '\n'{
                        out_lines.last_mut().unwrap().push(' ');
                    }
                },
                TokenType::Operator => {
                    if first_on_line{
                        first_on_line = false;
                        let extra_indent = if chunk.len() == 1 && (chunk[0] == '*' || chunk[0] == '.'|| chunk[0] == '&'){0}else{4};
                        output_indent(&mut out_lines, expected_indent + extra_indent);
                    }
                    if chunk.len() == 1{
                        if chunk[0] == '<'{
                            paren_stack.last_mut().unwrap().angle_counter += 1;
                        }
                        else if chunk[0] == '>'{
                            let last = paren_stack.last_mut().unwrap();
                            last.angle_counter = last.angle_counter.max(1) - 1;
                        }
                        else if chunk[0] != '&'{
                            paren_stack.last_mut().unwrap().angle_counter = 0
                        }
                    }
                    else{
                        paren_stack.last_mut().unwrap().angle_counter = 0
                    }
                    let last_line = out_lines.last_mut().unwrap();
                    if chunk.len() == 1 && (chunk[0] == '!' || chunk[0] == '|' || chunk[0] == '&' || chunk[0] == '*' || chunk[0] == '.' || chunk[0] == '<' || chunk[0] == '>'){
                        last_line.append(&mut chunk);
                    }
                    else{
                        if last_line.len() > 0 && *last_line.last().unwrap() != ' '{
                            if state.next != ' ' && state.next != '\n'{
                                last_line.push(' ');
                            }
                        }
                        last_line.append(&mut chunk);
                        if state.next != ' ' && state.next != '\n'{
                            last_line.push(' ');
                        }
                    }
                },
                TokenType::BuiltinType | TokenType::TypeName => {
                    first_after_open = false;
                    if first_on_line{
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    out_lines.last_mut().unwrap().append(&mut chunk);
                },
                _ => { 
                    paren_stack.last_mut().unwrap().angle_counter = 0;
                    first_after_open = false;
                    if first_on_line{
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    out_lines.last_mut().unwrap().append(&mut chunk);
                },
            }
            last_token = token_type;
            chunk.truncate(0);
        }
        // lets do a diff from top, and bottom
        let mut top_row = 0;
        while top_row < text_buffer.lines.len() && top_row < out_lines.len() && text_buffer.lines[top_row] == out_lines[top_row]{
            top_row += 1;
        }
        
        let mut bottom_row_old = text_buffer.lines.len();
        let mut bottom_row_new = out_lines.len();
        while bottom_row_old > top_row && bottom_row_new > top_row && text_buffer.lines[bottom_row_old - 1] == out_lines[bottom_row_new - 1]{
            bottom_row_old -= 1;
            bottom_row_new -= 1;
        }
        // alright we now have a line range to replace.
        if top_row != bottom_row_new{
            let changed = out_lines.splice(top_row..(bottom_row_new + 1).min(out_lines.len()), vec![]).collect();
            self.code_editor.cursors.replace_lines_formatted(top_row, bottom_row_old + 1, changed, text_buffer);
        }
    }
    
    pub fn draw_rust_editor(&mut self, cx:&mut Cx, text_buffer:&TextBuffer){
        if let Err(()) = self.code_editor.begin_code_editor(cx, text_buffer){
            return
        }
        
        if self.set_key_focus_on_draw{
            self.set_key_focus_on_draw = false;
            self.code_editor.set_key_focus(cx);
        }
        
        let mut state = TokenizerState::new(text_buffer);
        let mut rust_tok = RustTokenizer::new();
        let mut chunk = Vec::new();
        
        loop{
            let token_type = rust_tok.next_token(&mut state, &mut chunk, &self.code_editor.token_chunks);
            if token_type == TokenType::Eof{
                self.code_editor.draw_chunk(cx, &chunk, state.next, state.offset, TokenType::Whitespace, &text_buffer.messages.cursors);
                break
            }
            else{
                self.code_editor.draw_chunk(cx, &chunk, state.next, state.offset, token_type, &text_buffer.messages.cursors);
            }
            chunk.truncate(0);
        }
        
        self.code_editor.end_code_editor(cx, text_buffer);
    }
}

pub struct RustTokenizer{
    pub comment_single:bool,
    pub comment_depth:usize
}

impl RustTokenizer{
    fn new() -> RustTokenizer{
        RustTokenizer{
            comment_single:false,
            comment_depth:0
        }
    }
    
    fn next_token<'a>(&mut self, state:&mut TokenizerState<'a>, chunk:&mut Vec<char>, token_chunks:&Vec<TokenChunk>) -> TokenType{
        if self.comment_depth > 0{ // parse comments
            loop{
                if state.next == '\0'{
                    self.comment_depth = 0;
                    return TokenType::CommentChunk
                }
                if state.next == '/'{
                    chunk.push(state.next);
                    state.advance();
                    if state.next == '*'{
                        chunk.push(state.next);
                        state.advance();
                        self.comment_depth += 1;
                    }
                }
                else if state.next == '*'{
                    chunk.push(state.next);
                    state.advance();
                    if state.next == '/'{
                        self.comment_depth -= 1;
                        chunk.push(state.next);
                        state.advance();
                        if self.comment_depth == 0{
                            return TokenType::CommentMultiEnd
                        }
                    }
                }
                else if state.next == '\n'{
                    if self.comment_single {
                        self.comment_depth = 0;
                    }
                    // output current line
                    if chunk.len()>0{
                        return TokenType::CommentChunk
                    }
                    
                    chunk.push(state.next);
                    state.advance();
                    return TokenType::Newline
                }
                else if state.next == ' '{
                    if chunk.len()>0{
                        return TokenType::CommentChunk
                    }
                    while state.next == ' '{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Whitespace
                }
                else{
                    chunk.push(state.next);
                    state.advance();
                }
            }
        }
        else{
            state.advance_with_cur();
            match state.cur{
                '\0' => { // eof insert a terminating space and end
                    chunk.push(' ');
                    return TokenType::Eof
                },
                '\n' => {
                    chunk.push('\n');
                    return TokenType::Newline
                },
                ' ' | '\t' => { // eat as many spaces as possible
                    chunk.push(state.cur);
                    while state.next == ' '{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Whitespace;
                },
                '/' => { // parse comment
                    chunk.push(state.cur);
                    if state.next == '/'{
                        chunk.push(state.next);
                        state.advance();
                        self.comment_depth = 1;
                        self.comment_single = true;
                        return TokenType::CommentLine;
                    }
                    else if state.next == '*'{ // start parsing a multiline comment
                        //let mut comment_depth = 1;
                        chunk.push(state.next);
                        state.advance();
                        self.comment_single = false;
                        self.comment_depth = 1;
                        return TokenType::CommentMultiBegin;
                    }
                    else{
                        if state.next == '='{
                            chunk.push(state.next);
                            state.advance();
                        }
                        return TokenType::Operator;
                    }
                },
                '\'' => { // parse char literal or lifetime annotation
                    chunk.push(state.cur);
                    
                    if Self::parse_rust_escape_char(state, chunk){ // escape char or unicode
                        if state.next == '\''{ // parsed to closing '
                            chunk.push(state.next);
                            state.advance();
                            return TokenType::String;
                        }
                        else{
                            return TokenType::TypeName;
                        }
                    }
                    else{ // parse a single char or lifetime
                        let offset = state.offset;
                        if Self::parse_rust_ident_tail(state, chunk) && ((state.offset - offset) > 1 || state.next != '\''){
                            return TokenType::Keyword;
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
                            return TokenType::String;
                        }
                        else{
                            return TokenType::String;
                        }
                    }
                },
                '"' => { // parse string
                    chunk.push(state.cur);
                    state.prev = '\0';
                    while state.next != '\0' && state.next != '\n'{
                        if state.next != '"' || state.prev != '\\' && state.cur == '\\' && state.next == '"'{
                            chunk.push(state.next);
                            state.advance_with_prev();
                        }
                        else{
                            chunk.push(state.next);
                            state.advance();
                            break;
                        }
                    };
                    return TokenType::String;
                },
                '0'...'9' => { // try to parse numbers
                    chunk.push(state.cur);
                    Self::parse_rust_number_tail(state, chunk);
                    return TokenType::Number;
                },
                ':' => {
                    chunk.push(state.cur);
                    if state.next == ':'{
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Namespace;
                    }
                    return TokenType::Colon;
                },
                '*' => {
                    chunk.push(state.cur);
                    if state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Operator;
                    }
                    else if state.next == '/'{
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Unexpected;
                    }
                    else{
                        return TokenType::Operator;
                    }
                },
                '+' => {
                    chunk.push(state.cur);
                    if state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '-' => {
                    chunk.push(state.cur);
                    if state.next == '>' || state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '=' => {
                    chunk.push(state.cur);
                    if state.next == '>' || state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    
                    return TokenType::Operator;
                },
                '.' => {
                    chunk.push(state.cur);
                    if state.next == '.' {
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Splat;
                    }
                    return TokenType::Operator;
                },
                ';' => {
                    chunk.push(state.cur);
                    if state.next == '.' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Delimiter;
                },
                '&' => {
                    chunk.push(state.cur);
                    if state.next == '&' || state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '|' => {
                    chunk.push(state.cur);
                    if state.next == '|' || state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '!' => {
                    chunk.push(state.cur);
                    if state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '<' => {
                    chunk.push(state.cur);
                    if state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '>' => {
                    chunk.push(state.cur);
                    if state.next == '='{
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                ',' => {
                    chunk.push(state.cur);
                    if state.next == '.' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Delimiter;
                },
                '(' | '{' | '[' => {
                    chunk.push(state.cur);
                    return TokenType::ParenOpen;
                },
                ')' | '}' | ']' => {
                    chunk.push(state.cur);
                    return TokenType::ParenClose;
                },
                '#' => {
                    chunk.push(state.cur);
                    return TokenType::Hash;
                },
                '_' => {
                    chunk.push(state.cur);
                    Self::parse_rust_ident_tail(state, chunk);
                    return TokenType::Identifier;
                },
                'a'...'z' => { // try to parse keywords or identifiers
                    chunk.push(state.cur);
                    let mut keyword_type = Self::parse_rust_lc_keyword(state, chunk);
                    
                    if Self::parse_rust_ident_tail(state, chunk){
                        keyword_type = KeywordType::None;
                    }
                    match keyword_type{
                        KeywordType::Normal => {
                            return TokenType::Keyword;
                        },
                        KeywordType::Flow => {
                            return TokenType::Flow;
                            //self.code_editor.set_indent_color(self.code_editor.colors.indent_line_flow);
                        },
                        KeywordType::BuiltinType => {
                            return TokenType::BuiltinType;
                        },
                        KeywordType::Fn => {
                            return TokenType::Fn;
                            //self.code_editor.set_indent_color(self.code_editor.colors.indent_line_fn);
                        },
                        KeywordType::Def => {
                            return TokenType::Def;
                            //self.code_editor.set_indent_color(self.code_editor.colors.indent_line_def);
                        },
                        KeywordType::Looping => {
                            return TokenType::Looping;
                            //self.code_editor.set_indent_color(self.code_editor.colors.indent_line_looping);
                        },
                        KeywordType::For => {
                            // check if we are first on a line
                            if token_chunks.len() < 2
                                || token_chunks[token_chunks.len() - 1].token_type == TokenType::Newline
                                || token_chunks[token_chunks.len() - 2].token_type == TokenType::Newline
                                && token_chunks[token_chunks.len() - 1].token_type == TokenType::Whitespace{
                                return TokenType::Looping;
                                //self.code_editor.set_indent_color(self.code_editor.colors.indent_line_looping);
                            }
                            else{
                                return TokenType::Keyword;
                                // self.code_editor.set_indent_color(self.code_editor.colors.indent_line_def);
                            }
                        },
                        KeywordType::None => {
                            //if state.next == '!'{
                            // chunk.push(state.next);
                            // state.advance();
                            // return TokenType::Call;
                            // }
                            //else
                            if state.next == '('{
                                return TokenType::Call;
                            }
                            else{
                                return TokenType::Identifier;
                            }
                        }
                    }
                },
                'A'...'Z' => {
                    chunk.push(state.cur);
                    let mut is_keyword = false;
                    if state.cur == 'S'{
                        if state.keyword(chunk, "elf"){
                            is_keyword = true;
                        }
                    }
                    if Self::parse_rust_ident_tail(state, chunk){
                        is_keyword = false;
                    }
                    if is_keyword{
                        return TokenType::Keyword;
                    }
                    else{
                        return TokenType::TypeName;
                        //if state.next == '{'{
                        // self.code_editor.set_indent_color(self.code_editor.colors.indent_line_def);
                        //}
                    }
                },
                _ => {
                    chunk.push(state.cur);
                    return TokenType::Operator;
                }
            }
        }
    }
    
    fn parse_rust_ident_tail<'a>(state:&mut TokenizerState<'a>, chunk:&mut Vec<char>) -> bool{
        let mut ret = false;
        while state.next_is_digit() || state.next_is_letter() || state.next == '_' || state.next == '$'{
            ret = true;
            chunk.push(state.next);
            state.advance();
        }
        ret
    }
    
    fn parse_rust_escape_char<'a>(state:&mut TokenizerState<'a>, chunk:&mut Vec<char>) -> bool{
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
            while state.next == '0' || state.next == '1' || state.next == '_'{
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
                else if state.keyword(chunk, "32"){
                }
                else if state.keyword(chunk, "64"){
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
                    if state.keyword(chunk, "32"){
                    }
                    else if state.keyword(chunk, "64"){
                    }
                }
            }
        }
    }
    
    fn parse_rust_lc_keyword<'a>(state:&mut TokenizerState<'a>, chunk:&mut Vec<char>) -> KeywordType{
        match state.cur{
            'a' => {
                if state.keyword(chunk, "s"){
                    return KeywordType::Normal
                }
            },
            'b' => {
                if state.keyword(chunk, "reak"){
                    return KeywordType::Flow
                }
                if state.keyword(chunk, "ool"){
                    return KeywordType::BuiltinType
                }
            },
            'c' => {
                if state.keyword(chunk, "o"){
                    if state.keyword(chunk, "nst"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk, "ntinue"){
                        return KeywordType::Flow
                    }
                }
                else if state.keyword(chunk, "rate"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk, "har"){
                    return KeywordType::BuiltinType
                }
            },
            'e' => {
                if state.keyword(chunk, "lse"){
                    return KeywordType::Flow
                }
                else if state.keyword(chunk, "num"){
                    return KeywordType::Def
                }
                else if state.keyword(chunk, "xtern"){
                    return KeywordType::Normal
                }
            },
            'f' => {
                if state.keyword(chunk, "alse"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk, "n"){
                    return KeywordType::Fn
                }
                else if state.keyword(chunk, "or"){
                    return KeywordType::For
                }
                else if state.keyword(chunk, "32"){
                    return KeywordType::BuiltinType
                }
                else if state.keyword(chunk, "64"){
                    return KeywordType::BuiltinType
                }
            },
            'i' => {
                if state.keyword(chunk, "f"){
                    return KeywordType::Flow
                }
                else if state.keyword(chunk, "mpl"){
                    return KeywordType::Def
                }
                else if state.keyword(chunk, "n"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk, "8"){
                    return KeywordType::BuiltinType
                }
                else if state.keyword(chunk, "16"){
                    return KeywordType::BuiltinType
                }
                else if state.keyword(chunk, "32"){
                    return KeywordType::BuiltinType
                }
                else if state.keyword(chunk, "64"){
                    return KeywordType::BuiltinType
                }
            },
            'l' => {
                if state.keyword(chunk, "et"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk, "oop"){
                    return KeywordType::Looping
                }
            },
            'm' => {
                if state.keyword(chunk, "atch"){
                    return KeywordType::Flow
                }
                else if state.keyword(chunk, "o"){
                    if state.keyword(chunk, "d"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk, "ve"){
                        return KeywordType::Normal
                    }
                }
                else if state.keyword(chunk, "ut"){
                    return KeywordType::Normal
                }
            },
            'p' => { // pub
                if state.keyword(chunk, "ub"){
                    return KeywordType::Normal
                }
            },
            'r' => {
                if state.keyword(chunk, "e"){
                    if state.keyword(chunk, "f"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk, "turn"){
                        return KeywordType::Flow
                    }
                }
            },
            's' => {
                if state.keyword(chunk, "elf"){
                    return KeywordType::Normal
                }
                if state.keyword(chunk, "uper"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk, "t"){
                    if state.keyword(chunk, "atic"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk, "ruct"){
                        return KeywordType::Def
                    }
                }
            },
            't' => {
                if state.keyword(chunk, "ype"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk, "r"){
                    if state.keyword(chunk, "ait"){
                        return KeywordType::Def
                    }
                    else if state.keyword(chunk, "ue"){
                        return KeywordType::Normal
                    }
                }
            },
            'u' => { // use
                if state.keyword(chunk, "se"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk, "nsafe"){
                    return KeywordType::Normal
                }
                else if state.keyword(chunk, "8"){
                    return KeywordType::BuiltinType
                }
                else if state.keyword(chunk, "16"){
                    return KeywordType::BuiltinType
                }
                else if state.keyword(chunk, "32"){
                    return KeywordType::BuiltinType
                }
                else if state.keyword(chunk, "64"){
                    return KeywordType::BuiltinType
                }
            },
            'w' => { // use
                if state.keyword(chunk, "h"){
                    if state.keyword(chunk, "ere"){
                        return KeywordType::Normal
                    }
                    else if state.keyword(chunk, "ile"){
                        return KeywordType::Looping
                    }
                }
            },
            
            _ => {}
        }
        KeywordType::None
    }
}



enum KeywordType{
    None,
    Normal,
    Flow,
    Fn,
    Looping,
    For,
    Def,
    BuiltinType
}