use render::*;

use crate::textbuffer::*;
use crate::codeeditor::*;

#[derive(Clone)]
pub struct RustEditor {
    pub code_editor: CodeEditor,
}

impl RustEditor {
    pub fn proto(cx: &mut Cx) -> Self {
        let editor = Self {
            code_editor: CodeEditor::proto(cx),
        };
        //tab.animator.default = tab.anim_default(cx);
        editor
    }
      
    pub fn handle_rust_editor(&mut self, cx: &mut Cx, event: &mut Event, text_buffer: &mut TextBuffer) -> CodeEditorEvent {
        let ce = self.code_editor.handle_code_editor(cx, event, text_buffer);
        match ce {
            CodeEditorEvent::AutoFormat => {
                let formatted = RustTokenizer::auto_format(text_buffer, false).out_lines;
                self.code_editor.cursors.replace_lines_formatted(formatted, text_buffer);
                self.code_editor.view.redraw_view_area(cx);
            },
            _ => ()
        }
        ce
    }
    
    pub fn draw_rust_editor(&mut self, cx: &mut Cx, text_buffer: &mut TextBuffer) {
        if text_buffer.needs_token_chunks() && text_buffer.lines.len() >0 {
            let mut state = TokenizerState::new(&text_buffer.lines);
            let mut tokenizer = RustTokenizer::new();
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

pub struct RustTokenizer {
    pub comment_single: bool,
    pub comment_depth: usize
}

impl RustTokenizer {
    pub fn new() -> RustTokenizer {
        RustTokenizer {
            comment_single: false,
            comment_depth: 0
        }
    }
    
    pub fn next_token<'a>(&mut self, state: &mut TokenizerState<'a>, chunk: &mut Vec<char>, token_chunks: &Vec<TokenChunk>) -> TokenType {
        let start = chunk.len();
        //chunk.truncate(0);
        if self.comment_depth >0 { // parse comments
            loop {
                if state.next == '\0' {
                    self.comment_depth = 0;
                    return TokenType::CommentChunk
                }
                if state.next == '/' {
                    chunk.push(state.next);
                    state.advance();
                    if state.next == '*' {
                        chunk.push(state.next);
                        state.advance();
                        self.comment_depth += 1;
                    }
                }
                else if state.next == '*' {
                    chunk.push(state.next);
                    state.advance();
                    if state.next == '/' {
                        self.comment_depth -= 1;
                        chunk.push(state.next);
                        state.advance();
                        if self.comment_depth == 0 {
                            return TokenType::CommentMultiEnd
                        }
                    }
                }
                else if state.next == '\n' {
                    if self.comment_single {
                        self.comment_depth = 0;
                    }
                    // output current line
                    if (chunk.len() - start)>0 {
                        return TokenType::CommentChunk
                    }
                    
                    chunk.push(state.next);
                    state.advance();
                    return TokenType::Newline
                }
                else if state.next == ' ' {
                    if (chunk.len() - start)>0 {
                        return TokenType::CommentChunk
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
        else {
            state.advance_with_cur();
            match state.cur {
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
                    while state.next == ' ' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Whitespace;
                },
                '/' => { // parse comment
                    chunk.push(state.cur);
                    if state.next == '/' {
                        chunk.push(state.next);
                        state.advance();
                        self.comment_depth = 1;
                        self.comment_single = true;
                        return TokenType::CommentLine;
                    }
                    if state.next == '*' { // start parsing a multiline comment
                        //let mut comment_depth = 1;
                        chunk.push(state.next);
                        state.advance();
                        self.comment_single = false;
                        self.comment_depth = 1;
                        return TokenType::CommentMultiBegin;
                    }
                    if state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '\'' => { // parse char literal or lifetime annotation
                    chunk.push(state.cur);
                    
                    if Self::parse_rust_escape_char(state, chunk) { // escape char or unicode
                        if state.next == '\'' { // parsed to closing '
                            chunk.push(state.next);
                            state.advance();
                            return TokenType::String;
                        }
                        return TokenType::TypeName;
                    }
                    else { // parse a single char or lifetime
                        let offset = state.offset;
                        let (is_ident, _) = Self::parse_rust_ident_tail(state, chunk);
                        if is_ident && ((state.offset - offset) >1 || state.next != '\'') {
                            return TokenType::TypeName;
                        }
                        if state.next != '\n' {
                            if (state.offset - offset) == 0 { // not an identifier char
                                chunk.push(state.next);
                                state.advance();
                            }
                            if state.next == '\'' { // lifetime identifier
                                chunk.push(state.next);
                                state.advance();
                            }
                            return TokenType::String;
                        }
                        return TokenType::String;
                    }
                },
                '"' => { // parse string
                    chunk.push(state.cur);
                    state.prev = '\0';
                    while state.next != '\0' && state.next != '\n' {
                        if state.next != '"' || state.prev != '\\' && state.cur == '\\' && state.next == '"' {
                            chunk.push(state.next);
                            state.advance_with_prev();
                        }
                        else {
                            chunk.push(state.next);
                            state.advance();
                            break;
                        }
                    };
                    return TokenType::String;
                },
                '0'..='9' => { // try to parse numbers
                    chunk.push(state.cur);
                    Self::parse_rust_number_tail(state, chunk);
                    return TokenType::Number;
                },
                ':' => {
                    chunk.push(state.cur);
                    if state.next == ':' {
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Namespace;
                    }
                    return TokenType::Colon;
                },
                '*' => {
                    chunk.push(state.cur);
                    if state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Operator;
                    }
                    if state.next == '/' {
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Unexpected;
                    }
                    return TokenType::Operator;
                },
                '^' => {
                    chunk.push(state.cur);
                    if state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '+' => {
                    chunk.push(state.cur);
                    if state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '-' => {
                    chunk.push(state.cur);
                    if state.next == '>' || state.next == '=' {
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
                        if state.next == '=' {
                            chunk.push(state.next);
                            state.advance();
                            return TokenType::Splat;
                        }
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
                    if state.next == '&' || state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '|' => {
                    chunk.push(state.cur);
                    if state.next == '|' || state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '!' => {
                    chunk.push(state.cur);
                    if state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '<' => {
                    chunk.push(state.cur);
                    if state.next == '=' || state.next == '<' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '>' => {
                    chunk.push(state.cur);
                    if state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                ',' => {
                    chunk.push(state.cur);
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
                    if state.next == '(' {
                        return TokenType::Call;
                    }
                    return TokenType::Identifier;
                },
                'a'..='z' => { // try to parse keywords or identifiers
                    chunk.push(state.cur);
                    
                    let keyword_type = Self::parse_rust_lc_keyword(state, chunk, token_chunks);
                    let (is_ident, _) = Self::parse_rust_ident_tail(state, chunk);
                    if is_ident {
                        if state.next == '(' {
                            return TokenType::Call;
                        }
                        return TokenType::Identifier;
                    }
                    else {
                        return keyword_type
                    }
                },
                'A'..='Z' => {
                    chunk.push(state.cur);
                    let mut is_keyword = false;
                    if state.cur == 'S' {
                        if state.keyword(chunk, "elf") {
                            is_keyword = true;
                        }
                    }
                    let (is_ident, has_underscores) = Self::parse_rust_ident_tail(state, chunk);
                    if is_ident {
                        is_keyword = false;
                    }
                    if has_underscores {
                        return TokenType::ThemeName;
                    }
                    if is_keyword {
                        return TokenType::Keyword;
                    }
                    return TokenType::TypeName;
                },
                _ => {
                    chunk.push(state.cur);
                    return TokenType::Operator;
                }
            }
        }
    }
    
    fn parse_rust_ident_tail<'a>(state: &mut TokenizerState<'a>, chunk: &mut Vec<char>) -> (bool,bool) {
        let mut ret = false;
        let mut has_underscores = false;
        while state.next_is_digit() || state.next_is_letter() || state.next == '_' || state.next == '$' {
            if state.next == '_'{
                has_underscores = true;
            }
            ret = true;
            chunk.push(state.next);
            state.advance();
        }
        (ret, has_underscores)
    }
    
    
    fn parse_rust_escape_char<'a>(state: &mut TokenizerState<'a>, chunk: &mut Vec<char>) -> bool {
        if state.next == '\\' {
            chunk.push(state.next);
            state.advance();
            if state.next == 'u' {
                chunk.push(state.next);
                state.advance();
                if state.next == '{' {
                    chunk.push(state.next);
                    state.advance();
                    while state.next_is_hex() {
                        chunk.push(state.next);
                        state.advance();
                    }
                    if state.next == '}' {
                        chunk.push(state.next);
                        state.advance();
                    }
                }
            }
            else if state.next != '\n' && state.next != '\0' {
                // its a single char escape TODO limit this to valid escape chars
                chunk.push(state.next);
                state.advance();
            }
            return true
        }
        return false
    }
    fn parse_rust_number_tail<'a>(state: &mut TokenizerState<'a>, chunk: &mut Vec<char>) {
        if state.next == 'x' { // parse a hex number
            chunk.push(state.next);
            state.advance();
            while state.next_is_hex() || state.next == '_' {
                chunk.push(state.next);
                state.advance();
            }
        }
        else if state.next == 'b' { // parse a binary
            chunk.push(state.next);
            state.advance();
            while state.next == '0' || state.next == '1' || state.next == '_' {
                chunk.push(state.next);
                state.advance();
            }
        }
        else if state.next == 'o' { // parse a octal
            chunk.push(state.next);
            state.advance();
            while state.next == '0' || state.next == '1' || state.next == '2'
                || state.next == '3' || state.next == '4' || state.next == '5'
                || state.next == '6' || state.next == '7' || state.next == '_' {
                chunk.push(state.next);
                state.advance();
            }
        }
        else {
            while state.next_is_digit() || state.next == '_' {
                chunk.push(state.next);
                state.advance();
            }
            if state.next == 'u' || state.next == 'i' {
                chunk.push(state.next);
                state.advance();
                if state.keyword(chunk, "8") {
                }
                else if state.keyword(chunk, "16") {
                }
                else if state.keyword(chunk, "32") {
                }
                else if state.keyword(chunk, "64") {
                }
            }
            else if state.next == '.' || state.next == 'f' || state.next == 'e' || state.next == 'E' {
                if state.next == '.' || state.next == 'f' {
                    chunk.push(state.next);
                    state.advance();
                    while state.next_is_digit() || state.next == '_' {
                        chunk.push(state.next);
                        state.advance();
                    }
                }
                if state.next == 'E' || state.next == 'e' {
                    chunk.push(state.next);
                    state.advance();
                    if state.next == '+' || state.next == '-'{
                        chunk.push(state.next);
                        state.advance();
                        while state.next_is_digit() || state.next == '_' {
                            chunk.push(state.next);
                            state.advance();
                        }
                    }
                    else {
                        return
                    }
                }
                if state.next == 'f' { // the f32, f64 postfix
                    chunk.push(state.next);
                    state.advance();
                    if state.keyword(chunk, "32") {
                    }
                    else if state.keyword(chunk, "64") {
                    }
                }
            }
        }
    }
    
    fn parse_rust_lc_keyword<'a>(state: &mut TokenizerState<'a>, chunk: &mut Vec<char>, token_chunks: &Vec<TokenChunk>) -> TokenType {
        match state.cur {
            'a' => {
                if state.keyword(chunk, "s") {
                    return TokenType::Keyword
                }
            },
            'b' => {
                if state.keyword(chunk, "reak") {
                    return TokenType::Flow
                }
                if state.keyword(chunk, "ool") {
                    return TokenType::BuiltinType
                }
            },
            'c' => {
                if state.keyword(chunk, "on") {
                    if state.keyword(chunk, "st") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "tinue") {
                        return TokenType::Flow
                    }
                }
                if state.keyword(chunk, "rate") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "har") {
                    return TokenType::BuiltinType
                }
            },
            'd' =>{
                if state.keyword(chunk, "yn") {
                    return TokenType::Keyword
                } 
            },
            'e' => {
                if state.keyword(chunk, "lse") {
                    return TokenType::Flow
                }
                if state.keyword(chunk, "num") {
                    return TokenType::TypeDef
                }
                if state.keyword(chunk, "xtern") {
                    return TokenType::Keyword
                }
            },
            'f' => {
                if state.keyword(chunk, "alse") {
                    return TokenType::Bool
                }
                if state.keyword(chunk, "n") {
                    return TokenType::Fn
                }
                if state.keyword(chunk, "or") {
                    // check if we are first on a line
                    if token_chunks.len() <2
                        || token_chunks[token_chunks.len() - 1].token_type == TokenType::Newline
                        || token_chunks[token_chunks.len() - 2].token_type == TokenType::Newline
                        && token_chunks[token_chunks.len() - 1].token_type == TokenType::Whitespace {
                        return TokenType::Looping;
                        //self.code_editor.set_indent_color(self.code_editor.colors.indent_line_looping);
                    }
                    
                    return TokenType::Keyword;
                    // self.code_editor.set_indent_color(self.code_editor.colors.indent_line_def);
                }
                
                if state.keyword(chunk, "32") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "64") {
                    return TokenType::BuiltinType
                }
            },
            'i' => {
                if state.keyword(chunk, "f") {
                    return TokenType::Flow
                }
                if state.keyword(chunk, "mpl") {
                    return TokenType::TypeDef
                }
                if state.keyword(chunk, "size") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "n") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "8") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "16") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "32") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "64") {
                    return TokenType::BuiltinType
                }
            },
            'l' => {
                if state.keyword(chunk, "et") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "oop") {
                    return TokenType::Looping
                }
            },
            'm' => {
                if state.keyword(chunk, "atch") {
                    return TokenType::Flow
                }
                if state.keyword(chunk, "ut") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "o") {
                    if state.keyword(chunk, "d") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "ve") {
                        return TokenType::Keyword
                    }
                }
            },
            'p' => { // pub
                if state.keyword(chunk, "ub") {
                    return TokenType::Keyword
                }
            },
            'r' => {
                if state.keyword(chunk, "e") {
                    if state.keyword(chunk, "f") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "turn") {
                        return TokenType::Flow
                    }
                }
            },
            's' => {
                if state.keyword(chunk, "elf") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "uper") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "t") {
                    if state.keyword(chunk, "atic") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "r") {
                        if state.keyword(chunk, "uct"){
                            return TokenType::TypeDef
                        }
                        return TokenType::BuiltinType
                    }
                }
            },
            't' => {
                if state.keyword(chunk, "ype") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "r") {
                    if state.keyword(chunk, "ait") {
                        return TokenType::TypeDef
                    }
                    if state.keyword(chunk, "ue") {
                        return TokenType::Bool
                    }
                }
            },
            'u' => { // use
                
                if state.keyword(chunk, "nsafe") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "8") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "16") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "32") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "64") {
                    return TokenType::BuiltinType
                }
                if state.keyword(chunk, "s") {
                    if state.keyword(chunk, "ize") {
                        return TokenType::BuiltinType
                    }
                    if state.keyword(chunk, "e") {
                        return TokenType::Keyword
                    }
                }
            },
            'w' => { // use
                if state.keyword(chunk, "h") {
                    if state.keyword(chunk, "ere") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "ile") {
                        return TokenType::Looping
                    }
                }
            },
            
            _ => {}
        }
        if state.next == '(' {
            return TokenType::Call;
        }
        else {
            return TokenType::Identifier;
        }
    }
    
    // because rustfmt is such an insane shitpile to compile or use as a library, here is a stupid version.
    pub fn auto_format(text_buffer: &mut TextBuffer, force_newlines:bool) -> FormatOutput {
        
        // extra spacey setting that rustfmt seems to do, but i don't like
        let extra_spacey = false;
        let pre_spacey = true;
        
        let mut out = FormatOutput::new();
        let mut tp = TokenParser::new(&text_buffer.flat_text, &text_buffer.token_chunks);
        
        struct ParenStack {
            expecting_newlines: bool,
            expected_indent: usize,
            angle_counter: usize
        }
        
        let mut paren_stack: Vec<ParenStack> = Vec::new();
        
        paren_stack.push(ParenStack {
            expecting_newlines: true,
            expected_indent: 0,
            angle_counter: 0
        });
        out.new_line();
        
        let mut first_on_line = true;
        let mut first_after_open = false;
        let mut expected_indent = 0;
        let mut is_unary_operator = true;
        let mut in_multline_comment = false;
        let mut in_singleline_comment = false;
        
        while tp.advance() {
            
            match tp.cur_type() {
                TokenType::Whitespace => {
                    if in_singleline_comment || in_multline_comment {
                        out.extend(tp.cur_chunk());
                    }
                    else if !first_on_line && tp.next_type() != TokenType::Newline
                        && tp.prev_type() != TokenType::ParenOpen
                        && tp.prev_type() != TokenType::Namespace
                        && tp.prev_type() != TokenType::Delimiter
                        && (tp.prev_type() != TokenType::Operator || (tp.prev_char() == '>' || tp.prev_char() == '<')) {
                        out.add_space();
                    }
                },
                TokenType::Newline => {
                    in_singleline_comment = false;
                    //paren_stack.last_mut().unwrap().angle_counter = 0;
                    if  in_singleline_comment || in_multline_comment{
                        out.new_line();
                        first_on_line = true;
                    }
                    else{
                        if first_on_line {
                            out.indent(expected_indent);
                        }
                        else {
                            out.strip_space();
                        }
                        if first_after_open {
                            paren_stack.last_mut().unwrap().expecting_newlines = true;
                            expected_indent += 4;
                        }
                        if paren_stack.last_mut().unwrap().expecting_newlines { // only insert when expecting newlines
                            first_after_open = false;
                            out.new_line();
                            first_on_line = true;
                        }
                    }
                },
                TokenType::Eof => {break},
                TokenType::ParenOpen => {
                    if first_on_line {
                        out.indent(expected_indent);
                    }
                    
                    paren_stack.push(ParenStack {
                        expecting_newlines: force_newlines,
                        expected_indent: expected_indent,
                        angle_counter: 0
                    });
                    first_after_open = true;
                    is_unary_operator = true;
                    
                    let is_curly = tp.cur_char() == '{';
                    if tp.cur_char() == '(' && (
                        tp.prev_type() == TokenType::Flow || tp.prev_type() == TokenType::Looping || tp.prev_type() == TokenType::Keyword
                    ) {
                        out.add_space();
                    }
                    if pre_spacey && is_curly && !first_on_line && tp.prev_type() != TokenType::Namespace {
                        if tp.prev_char() != ' ' && tp.prev_char() != '{'
                            && tp.prev_char() != '[' && tp.prev_char() != '(' && tp.prev_char() != ':' {
                            out.add_space();
                        }
                    }
                    else if !pre_spacey {
                        out.strip_space();
                    }
                    
                    out.extend(tp.cur_chunk());
                    
                    if extra_spacey && is_curly && tp.next_type() != TokenType::Newline {
                        out.add_space();
                    }
                    first_on_line = false;
                },
                TokenType::ParenClose => {
                    
                    out.strip_space();
                    
                    let expecting_newlines = paren_stack.last().unwrap().expecting_newlines;
                    
                    if extra_spacey && tp.cur_char() == '}' && !expecting_newlines {
                        out.add_space();
                    }
                    
                    first_after_open = false;
                    if !first_on_line && expecting_newlines { // we are expecting newlines!
                        out.new_line();
                        first_on_line = true;
                    }
                    
                    expected_indent = if paren_stack.len()>1 {
                        paren_stack.pop().unwrap().expected_indent
                    }
                    else {
                        0
                    };
                    if first_on_line {
                        first_on_line = false;
                        out.indent(expected_indent);
                    }
                    if tp.cur_char() == '}' {
                        is_unary_operator = true;
                    }
                    else {
                        is_unary_operator = false;
                    }
                    out.extend(tp.cur_chunk());
                },
                TokenType::CommentLine => {
                    in_singleline_comment = true;
                    if first_on_line {
                        first_on_line = false;
                        out.indent(expected_indent);
                    }
                    else {
                        out.add_space();
                    }
                    out.extend(tp.cur_chunk());
                },
                TokenType::CommentMultiBegin => {
                    in_multline_comment = true;
                    if first_on_line {
                        first_on_line = false;
                        out.indent(expected_indent);
                    }
                    out.extend(tp.cur_chunk());
                },
                TokenType::CommentChunk => {
                    if first_on_line {
                        first_on_line = false;
                    }
                    out.extend(tp.cur_chunk());
                },
                TokenType::CommentMultiEnd => {
                    in_multline_comment = false;
                    if first_on_line {
                        first_on_line = false;
                    }
                    out.extend(tp.cur_chunk());
                },
                TokenType::Colon => {
                    is_unary_operator = true;
                    out.strip_space();
                    out.extend(tp.cur_chunk());
                    if tp.next_type() != TokenType::Whitespace && tp.next_type() != TokenType::Newline {
                        out.add_space();
                    }
                },
                TokenType::Delimiter => {
                    if first_on_line {
                        first_on_line = false;
                        out.indent(expected_indent);
                    }
                    else {
                        out.strip_space();
                    }
                    out.extend(tp.cur_chunk());
                    if paren_stack.last_mut().unwrap().angle_counter == 0 // otherwise our generics multiline
                        && paren_stack.last().unwrap().expecting_newlines == true
                        && tp.next_type() != TokenType::Newline { // we are expecting newlines!
                        // scan forward to see if we really need a newline.
                        for next in (tp.index + 1)..tp.tokens.len() {
                            if tp.tokens[next].token_type == TokenType::Newline {
                                break;
                            }
                            if !tp.tokens[next].token_type.should_ignore() {
                                out.new_line();
                                first_on_line = true;
                                break;
                            }
                        }
                    }
                    else if tp.next_type() != TokenType::Newline {
                        out.add_space();
                    }
                    is_unary_operator = true;
                },
                TokenType::Operator => {
                    
                    // detect ++ and -- and execute insert or delete macros
                    
                    let mut is_closing_angle = false;
                    if tp.cur_char() == '<' {
                        paren_stack.last_mut().unwrap().angle_counter += 1;
                    }
                    else if tp.cur_char() == '>' {
                        let last = paren_stack.last_mut().unwrap();
                        last.angle_counter = last.angle_counter.max(1) - 1;
                        is_closing_angle = true;
                    }
                    else if tp.cur_char() != '&' && tp.cur_char() != '*' { // anything else resets the angle counter
                        paren_stack.last_mut().unwrap().angle_counter = 0
                    }
                    else {
                        paren_stack.last_mut().unwrap().angle_counter = 0
                    }
                    
                    if first_on_line {
                        first_on_line = false;
                        let extra_indent = if is_closing_angle || is_unary_operator {0}else {4};
                        out.indent(expected_indent + extra_indent);
                    }
                    
                    if (is_unary_operator && (tp.cur_char() == '-' || tp.cur_char() == '*' || tp.cur_char() == '&'))
                        || tp.cur_char() == '!' || tp.cur_char() == '.' || tp.cur_char() == '<' || tp.cur_char() == '>' {
                        out.extend(tp.cur_chunk());
                    }
                    else {
                        out.add_space();
                        out.extend(tp.cur_chunk());
                        if tp.next_type() != TokenType::Newline {
                            out.add_space();
                        }
                    }
                    
                    is_unary_operator = true;
                },
                TokenType::Identifier | TokenType::BuiltinType | TokenType::TypeName | TokenType::ThemeName=> { // these dont reset the angle counter
                    is_unary_operator = false;
                    
                    first_after_open = false;
                    if first_on_line {
                        first_on_line = false;
                        let extra_indent = if paren_stack.last_mut().unwrap().angle_counter >0 {4}else {0};
                        out.indent(expected_indent + extra_indent);
                    }
                    out.extend(tp.cur_chunk());
                },
                TokenType::Namespace => {
                    is_unary_operator = true;
                    
                    first_after_open = false;
                    if first_on_line {
                        first_on_line = false;
                        out.indent(expected_indent);
                    }
                    out.extend(tp.cur_chunk());
                },
                // these are followed by unary operators (some)
                TokenType::TypeDef | TokenType::Fn | TokenType::Hash | TokenType::Splat |
                TokenType::Keyword | TokenType::Flow | TokenType::Looping => {
                    is_unary_operator = true;
                    paren_stack.last_mut().unwrap().angle_counter = 0;
                    
                    first_after_open = false;
                    if first_on_line {
                        first_on_line = false;
                        out.indent(expected_indent);
                    }
                    out.extend(tp.cur_chunk());
                },
                // these are followeable by non unary operators
                TokenType::Call | TokenType::String | TokenType::Regex | TokenType::Number |
                TokenType::Bool | TokenType::Unexpected | TokenType::Error | TokenType::Warning | TokenType::Defocus=> {
                    is_unary_operator = false;
                    paren_stack.last_mut().unwrap().angle_counter = 0;
                    
                    first_after_open = false;
                    if first_on_line {
                        first_on_line = false;
                        out.indent(expected_indent);
                    }
                    out.extend(tp.cur_chunk());
                    
                },
            }
        };
        out
    }
}
