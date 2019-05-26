
use widget::*;
use crate::textbuffer::*;
use crate::textcursor::*;
use crate::codeeditor::*;

#[derive(Clone)]
pub struct JSEditor {
    pub code_editor: CodeEditor,
}

impl ElementLife for JSEditor {
    fn construct(&mut self, _cx: &mut Cx) {}
    fn destruct(&mut self, _cx: &mut Cx) {}
}

impl Style for JSEditor {
    fn style(cx: &mut Cx) -> Self {
        let rust_editor = Self {
            code_editor: CodeEditor {
                ..Style::style(cx)
            },
        };
        //tab.animator.default = tab.anim_default(cx);
        rust_editor
    }
}

#[derive(Clone, PartialEq)]
pub enum JSEditorEvent {
    None,
    Change
}

impl JSEditor {
    pub fn handle_js_editor(&mut self, cx: &mut Cx, event: &mut Event, text_buffer: &mut TextBuffer) -> CodeEditorEvent {
        let ce = self.code_editor.handle_code_editor(cx, event, text_buffer);
        match ce {
            CodeEditorEvent::AutoFormat => {
                let formatted = JSTokenizer::auto_format(text_buffer);
                self.code_editor.cursors.replace_lines_formatted(formatted, text_buffer);
                self.code_editor.view.redraw_view_area(cx);
            },
            _ => ()
        }
        ce
    }
    
    pub fn draw_js_editor(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {
        if let Err(()) = self.code_editor.begin_code_editor(cx, text_buffer) {
            return
        }
        
        let mut state = TokenizerState::new(text_buffer);
        let mut tokenizer = JSTokenizer::new();
        let mut chunk = Vec::new();
        
        loop {
            let token_type = tokenizer.next_token(&mut state, &mut chunk, &self.code_editor.token_chunks);
            self.code_editor.draw_chunk(cx, &chunk, state.next, state.offset, token_type.clone(), &text_buffer.messages.cursors);
            if token_type == TokenType::Eof {
                break
            }
            chunk.truncate(0);
        }
        
        self.code_editor.end_code_editor(cx, text_buffer);
    }
}

pub struct JSTokenizer {
    pub comment_single: bool,
    pub comment_depth: usize
}

impl JSTokenizer {
    pub fn new() -> JSTokenizer {
        JSTokenizer {
            comment_single: false,
            comment_depth: 0
        }
    }
    
    pub fn next_token<'a>(&mut self, state: &mut TokenizerState<'a>, chunk: &mut Vec<char>, token_chunks: &Vec<TokenChunk>) -> TokenType {
        if self.comment_depth >0 { // parse comments
            loop {
                if state.next == '\0' {
                    self.comment_depth = 0;
                    return TokenType::CommentChunk
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
                    if chunk.len()>0 {
                        return TokenType::CommentChunk
                    }
                    
                    chunk.push(state.next);
                    state.advance();
                    return TokenType::Newline
                }
                else if state.next == ' ' {
                    if chunk.len()>0 {
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
                '/' => { // parse comment or regexp
                    chunk.push(state.cur);
                    if state.next == '/' {
                        chunk.push(state.next);
                        state.advance();
                        self.comment_depth = 1;
                        self.comment_single = true;
                        return TokenType::CommentLine;
                    }
                    else if state.next == '*' { // start parsing a multiline comment
                        //let mut comment_depth = 1;
                        chunk.push(state.next);
                        state.advance();
                        self.comment_single = false;
                        self.comment_depth = 1;
                        return TokenType::CommentMultiBegin;
                    }
                    else {
                        let is_regexp = match TokenChunk::scan_last_token(&token_chunks) {
                            TokenType::ParenOpen | TokenType::Keyword | TokenType::Operator
                                | TokenType::Delimiter | TokenType::Colon | TokenType::Looping => true,
                            _ => false
                        };
                        if is_regexp {
                            while state.next != '\0' && state.next != '\n' {
                                if state.next != '/' || state.prev != '\\' && state.cur == '\\' && state.next == '/' {
                                    chunk.push(state.next);
                                    state.advance_with_prev();
                                }
                                else {
                                    chunk.push(state.next);
                                    state.advance();
                                    // lets see what characters we are followed by
                                    while state.next == 'g' || state.next == 'i' || state.next == 'm'
                                        || state.next == 's' || state.next == 'u' || state.next == 'y' {
                                        chunk.push(state.next);
                                        state.advance();
                                    }
                                    return TokenType::Regex;
                                }
                            };
                            return TokenType::Regex;
                        }
                        else if state.next == '=' {
                            chunk.push(state.next);
                            state.advance();
                        }
                        return TokenType::Operator;
                    }
                },
                '"' | '\'' => { // parse string
                    let end_char = state.cur;
                    chunk.push(state.cur);
                    state.prev = '\0';
                    while state.next != '\0' && state.next != '\n' {
                        if state.next == '\\' {
                            Self::parse_js_escape_char(state, chunk);
                        }
                        else if state.next != end_char || state.prev != '\\' && state.cur == '\\' && state.next == end_char {
                            chunk.push(state.next);
                            state.advance_with_prev();
                        }
                        else { // found the end
                            chunk.push(state.next);
                            state.advance();
                            return TokenType::String;
                        }
                    };
                    return TokenType::String;
                },
                '0'...'9' => { // try to parse numbers
                    chunk.push(state.cur);
                    Self::parse_js_number_tail(state, chunk);
                    return TokenType::Number;
                },
                ':' => {
                    chunk.push(state.cur);
                    return TokenType::Colon;
                },
                '*' => {
                    chunk.push(state.cur);
                    if state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Operator;
                    }
                    else if state.next == '/' {
                        chunk.push(state.next);
                        state.advance();
                        return TokenType::Unexpected;
                    }
                    else {
                        return TokenType::Operator;
                    }
                },
                '+' => {
                    chunk.push(state.cur);
                    if state.next == '=' || state.next == '+' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '-' => {
                    chunk.push(state.cur);
                    if state.next == '>' || state.next == '=' || state.next == '-' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    return TokenType::Operator;
                },
                '=' => {
                    chunk.push(state.cur);
                    if state.next == '>' {
                        chunk.push(state.next);
                        state.advance();
                    }
                    else if state.next == '=' {
                        chunk.push(state.next);
                        state.advance();
                        if state.next == '=' {
                            chunk.push(state.next);
                            state.advance();
                        }
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
                        if state.next == '=' {
                            chunk.push(state.next);
                            state.advance();
                        }
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
                    if state.next == '=' || state.next == '>' {
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
                '_' | '$' => {
                    chunk.push(state.cur);
                    Self::parse_js_ident_tail(state, chunk);
                    if state.next == '(' {
                        return TokenType::Call;
                    }
                    else {
                        return TokenType::Identifier;
                    }
                },
                'a'...'z' | 'A'...'Z' => { // try to parse keywords or identifiers
                    chunk.push(state.cur);
                    
                    let keyword_type = Self::parse_js_keyword(state, chunk, token_chunks);
                    
                    if Self::parse_js_ident_tail(state, chunk) {
                        if state.next == '(' {
                            return TokenType::Call;
                        }
                        else {
                            return TokenType::Identifier;
                        }
                    }
                    else {
                        return keyword_type
                    }
                },
                _ => {
                    chunk.push(state.cur);
                    return TokenType::Operator;
                }
            }
        }
    }
    
    fn parse_js_ident_tail<'a>(state: &mut TokenizerState<'a>, chunk: &mut Vec<char>) -> bool {
        let mut ret = false;
        while state.next_is_digit() || state.next_is_letter() || state.next == '_' || state.next == '$' {
            ret = true;
            chunk.push(state.next);
            state.advance();
        }
        ret
    }
    
    
    fn parse_js_escape_char<'a>(state: &mut TokenizerState<'a>, chunk: &mut Vec<char>) -> bool {
        if state.next == '\\' {
            chunk.push(state.next);
            state.advance();
            if state.next == 'u' {
                chunk.push(state.next);
                state.advance();
                // ! TODO LIMIT THIS TO MAX UNICODE
                while state.next_is_hex() {
                    chunk.push(state.next);
                    state.advance();
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
    fn parse_js_number_tail<'a>(state: &mut TokenizerState<'a>, chunk: &mut Vec<char>) {
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
        else {
            while state.next_is_digit() || state.next == '_' {
                chunk.push(state.next);
                state.advance();
            }
            if state.next == '.' {
                chunk.push(state.next);
                state.advance();
                // again eat as many numbers as possible
                while state.next_is_digit() || state.next == '_' {
                    chunk.push(state.next);
                    state.advance();
                }
            }
        }
    }
    
    fn parse_js_keyword<'a>(state: &mut TokenizerState<'a>, chunk: &mut Vec<char>, _token_chunks: &Vec<TokenChunk>) -> TokenType {
        match state.cur {
            'b' => {
                if state.keyword(chunk, "reak") {
                    return TokenType::Flow
                }
            },
            'c' => {
                if state.keyword(chunk, "ase") {
                    return TokenType::Flow
                }
                if state.keyword(chunk, "lass") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "o") {
                    if state.keyword(chunk, "nst") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "ntinue") {
                        return TokenType::Flow
                    }
                }
            },
            'd' => {
                if state.keyword(chunk, "o") {
                    return TokenType::Looping
                }
                if state.keyword(chunk, "e") {
                    if state.keyword(chunk, "bugger") {
                        return TokenType::Flow
                    }
                    if state.keyword(chunk, "fault") {
                        return TokenType::Flow
                    }
                    if state.keyword(chunk, "lete") {
                        return TokenType::Keyword
                    }
                }
            },
            'e' => {
                if state.keyword(chunk, "lse") {
                    return TokenType::Flow
                }
                if state.keyword(chunk, "num") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "xte") {
                    if state.keyword(chunk, "rn") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "nds") {
                        return TokenType::Keyword
                    }
                    return TokenType::TypeDef
                }
            },
            'f' => {
                if state.keyword(chunk, "alse") {
                    return TokenType::Bool
                }
                if state.keyword(chunk, "inally") {
                    return TokenType::Fn
                }
                if state.keyword(chunk, "or") {
                    return TokenType::Looping;
                }
                if state.keyword(chunk, "unction") {
                    return TokenType::Fn
                }
            },
            'g' => {
                if state.keyword(chunk, "et") {
                    return TokenType::Keyword
                }
            },
            'i' => {
                if state.keyword(chunk, "f") {
                    return TokenType::Flow
                }
                else if state.keyword(chunk, "mport") {
                    return TokenType::TypeDef
                }
                else if state.keyword(chunk, "in") {
                    if state.next_is_letter() || state.next_is_digit() {
                        if state.keyword(chunk, "stanceof") {
                            return TokenType::BuiltinType
                        }
                    }
                    else {
                        return TokenType::Keyword
                    }
                }
            },
            'l' => {
                if state.keyword(chunk, "et") {
                    return TokenType::Keyword
                }
            },
            'n' => {
                if state.keyword(chunk, "ew") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "ull") {
                    return TokenType::Keyword
                }
            },
            'r' => {
                if state.keyword(chunk, "eturn") {
                    return TokenType::Flow
                }
            },
            's' => {
                if state.keyword(chunk, "uper") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "witch") {
                    return TokenType::Flow
                }
                if state.keyword(chunk, "et") {
                    return TokenType::Keyword
                }
            },
            't' => {
                if state.keyword(chunk, "ry") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "ypeof") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "h") {
                    if state.keyword(chunk, "is") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "row") {
                        return TokenType::Flow
                    }
                }
            },
            'v' => { // use
                if state.keyword(chunk, "ar") {
                    return TokenType::Keyword
                }
                if state.keyword(chunk, "oid") {
                    return TokenType::Keyword
                }
            },
            'w' => { // use
                if state.keyword(chunk, "hile") {
                    return TokenType::Looping
                }
                if state.keyword(chunk, "ith") {
                    return TokenType::Keyword
                }
            },
            'u' => { // use
                if state.keyword(chunk, "ndefined") {
                    return TokenType::Keyword
                }
            },
            'y' => { // use
                if state.keyword(chunk, "ield") {
                    return TokenType::Flow
                }
            },
            'N' => {
                if state.keyword(chunk, "aN") {
                    return TokenType::Keyword
                }
            },
            'I' => {
                if state.keyword(chunk, "nfinity") {
                    return TokenType::Keyword
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
    
    // js autoformatter. nothing fancy.
    pub fn auto_format(text_buffer: &mut TextBuffer) -> Vec<Vec<char >> {
        
        let extra_spacey = false;
        let pre_spacey = true;
        let mut state = TokenizerState::new(text_buffer);
        let mut tokenizer = JSTokenizer::new();
        let mut out_lines: Vec<Vec<char >> = Vec::new();
        let mut chunk: Vec<char> = Vec::new();
        
        struct Token {
            chunk: Vec<char>,
            token_type: TokenType
        }
        
        let mut tokens: Vec<Token> = Vec::new();
        let mut token_chunks: Vec<TokenChunk> = Vec::new();
        
        let mut first_on_line = true;
        let mut first_after_open = false;
        
        struct ParenStack {
            expecting_newlines: bool,
            expected_indent: usize
        }
        
        let mut paren_stack: Vec<ParenStack> = Vec::new();
        
        paren_stack.push(ParenStack {
            expecting_newlines: true,
            expected_indent: 0,
        });
        out_lines.push(Vec::new());
        
        let mut expected_indent = 0;
        let mut is_unary_operator = true;
        let mut in_multline_comment = false;
        let mut in_singleline_comment = false;
        
        fn output_indent(out_lines: &mut Vec<Vec<char >>, indent_depth: usize) {
            let last_line = out_lines.last_mut().unwrap();
            for _ in 0..indent_depth {
                last_line.push(' ');
            }
        }
        
        fn strip_space(line: &mut Vec<char >) {
            if line.len()>0 && *line.last().unwrap() == ' ' {
                line.pop();
            }
        }
        
        loop {
            let token_type = tokenizer.next_token(&mut state, &mut chunk, &token_chunks);
            token_chunks.push(TokenChunk {
                offset: state.offset - chunk.len() - 1,
                pair_token: 0,
                len: chunk.len(),
                token_type: token_type.clone()
            });
            tokens.push(Token {
                chunk: chunk.clone(),
                token_type: token_type.clone()
            });
            chunk.truncate(0);
            if token_type == TokenType::Eof {
                break;
            }
        }
        
        for index in 0..tokens.len() {
            
            let chunk = &tokens[index].chunk;
            let token_type = &tokens[index].token_type;
            // look ahead and behind helpers
            let prev_type = if index > 0 {tokens[index - 1].token_type}else {TokenType::Unexpected};
            let next_type = if index < tokens.len() - 1 {tokens[index + 1].token_type}else {TokenType::Unexpected};
            let prev_char = if index > 0 && tokens[index - 1].chunk.len() == 1 {tokens[index - 1].chunk[0]}else {
                if index > 0 && tokens[index - 1].token_type == TokenType::Whitespace {' '} else {'\0'}
            };
            let cur_char = if tokens[index].chunk.len() == 1 {tokens[index].chunk[0]} else {'\0'};
            
            match token_type {
                TokenType::Whitespace => {
                    let last_line = out_lines.last_mut().unwrap();
                    if in_singleline_comment || in_multline_comment {
                        last_line.extend(chunk);
                    }
                    else if !first_on_line && next_type != TokenType::Newline
                        && prev_type != TokenType::ParenOpen
                        && prev_type != TokenType::Namespace
                        && prev_type != TokenType::Operator
                        && prev_type != TokenType::Delimiter {
                        out_lines.last_mut().unwrap().push(' ');
                    }
                },
                TokenType::Newline => {
                    in_singleline_comment = false;
                    //paren_stack.last_mut().unwrap().angle_counter = 0;
                    if first_on_line && !in_singleline_comment && !in_multline_comment {
                        output_indent(&mut out_lines, expected_indent);
                    }
                    else {
                        let last_line = out_lines.last_mut().unwrap();
                        if last_line.len() > 0 && *last_line.last().unwrap() == ' ' {
                            last_line.pop();
                        }
                    }
                    if first_after_open {
                        paren_stack.last_mut().unwrap().expecting_newlines = true;
                        expected_indent += 4;
                    }
                    if paren_stack.last_mut().unwrap().expecting_newlines { // only insert when expecting newlines
                        first_after_open = false;
                        out_lines.push(Vec::new());
                        first_on_line = true;
                    }
                },
                TokenType::Eof => {break},
                TokenType::ParenOpen => {
                    if first_on_line {
                        output_indent(&mut out_lines, expected_indent);
                    }
                    
                    paren_stack.push(ParenStack {
                        expecting_newlines: false,
                        expected_indent: expected_indent,
                    });
                    first_after_open = true;
                    is_unary_operator = true;
                    let last_line = out_lines.last_mut().unwrap();
                    let is_curly = cur_char == '{';
                    if cur_char == '(' && index > 0 && (
                        prev_type == TokenType::Flow || prev_type == TokenType::Looping || prev_type == TokenType::Keyword
                    ) {
                        last_line.push(' ');
                    }
                    if pre_spacey && is_curly && !first_on_line {
                        if prev_char != ' ' && prev_char != '{' && prev_char != '[' && prev_char != '(' && prev_char != ':' {
                            last_line.push(' ');
                        }
                    }
                    else if !pre_spacey {
                        strip_space(last_line);
                    }
                    
                    last_line.extend(chunk);
                    
                    if extra_spacey && is_curly && next_type != TokenType::Newline {
                        last_line.push(' ');
                    }
                    first_on_line = false;
                },
                TokenType::ParenClose => {
                    
                    let last_line = out_lines.last_mut().unwrap();
                    
                    strip_space(last_line);
                    
                    let expecting_newlines = paren_stack.last().unwrap().expecting_newlines;
                    
                    if extra_spacey && cur_char == '}' && !expecting_newlines && last_line.len()>0 && *last_line.last().unwrap() != ' ' {
                        last_line.push(' ');
                    }
                    
                    first_after_open = false;
                    if !first_on_line && expecting_newlines { // we are expecting newlines!
                        out_lines.push(Vec::new());
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
                        output_indent(&mut out_lines, expected_indent);
                    }
                    if cur_char == '}' {
                        is_unary_operator = true;
                    }
                    else {
                        is_unary_operator = false;
                    }
                    
                    out_lines.last_mut().unwrap().extend(chunk);
                },
                TokenType::CommentLine => {
                    in_singleline_comment = true;
                    if first_on_line {
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    else {
                        let last_line = out_lines.last_mut().unwrap();
                        if *last_line.last().unwrap() != ' ' {
                            last_line.push(' ');
                        }
                    }
                    out_lines.last_mut().unwrap().extend(chunk);
                },
                TokenType::CommentMultiBegin => {
                    in_multline_comment = true;
                    if first_on_line {
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    out_lines.last_mut().unwrap().extend(chunk);
                },
                TokenType::CommentChunk => {
                    if first_on_line {
                        first_on_line = false;
                    }
                    out_lines.last_mut().unwrap().extend(chunk);
                },
                TokenType::CommentMultiEnd => {
                    in_multline_comment = false;
                    if first_on_line {
                        first_on_line = false;
                    }
                    let last_line = out_lines.last_mut().unwrap();
                    last_line.extend(chunk);
                },
                TokenType::Colon => {
                    is_unary_operator = true;
                    let last_line = out_lines.last_mut().unwrap();
                    strip_space(last_line);
                    last_line.extend(chunk);
                    if next_type != TokenType::Whitespace && next_type != TokenType::Newline {
                        last_line.push(' ');
                    }
                },
                TokenType::Delimiter => {
                    if first_on_line {
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    else {
                        let last_line = out_lines.last_mut().unwrap();
                        strip_space(last_line);
                    }
                    out_lines.last_mut().unwrap().extend(chunk);
                    if paren_stack.last().unwrap().expecting_newlines == true
                        && next_type != TokenType::Newline { // we are expecting newlines!
                        // scan forward to see if we really need a newline.
                        for next in (index + 1)..tokens.len() {
                            if tokens[next].token_type == TokenType::Newline {
                                break;
                            }
                            if !tokens[next].token_type.should_ignore() {
                                out_lines.push(Vec::new());
                                first_on_line = true;
                                break;
                            }
                        }
                    }
                    else if next_type != TokenType::Newline {
                        out_lines.last_mut().unwrap().push(' ');
                    }
                    is_unary_operator = true;
                },
                TokenType::Operator => {
                    
                    if first_on_line {
                        first_on_line = false;
                        let extra_indent = if is_unary_operator {0}else {4};
                        output_indent(&mut out_lines, expected_indent + extra_indent);
                    }
                    
                    let last_line = out_lines.last_mut().unwrap();
                    if (is_unary_operator && (cur_char == '-' || cur_char == '*' || cur_char == '&'))
                        || cur_char == '.' || cur_char == '!' {
                        last_line.extend(chunk);
                    }
                    else {
                        if cur_char == '?' {
                            strip_space(last_line);
                        }
                        else if last_line.len() > 0 && *last_line.last().unwrap() != ' ' {
                            last_line.push(' ');
                        }
                        last_line.extend(chunk);
                        if next_type != TokenType::Newline {
                            last_line.push(' ');
                        }
                    }
                    
                    is_unary_operator = true;
                },
                // these are followed by unary operators (some)
                TokenType::TypeDef | TokenType::Fn | TokenType::Hash | TokenType::Splat | TokenType::Namespace |
                TokenType::Keyword | TokenType::Flow | TokenType::Looping => {
                    is_unary_operator = true;
                    
                    first_after_open = false;
                    if first_on_line {
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    out_lines.last_mut().unwrap().extend(chunk);
                },
                // these are followeable by non unary operators
                TokenType::Identifier | TokenType::BuiltinType | TokenType::TypeName |
                TokenType::Call | TokenType::String | TokenType::Regex | TokenType::Number |
                TokenType::Bool | TokenType::Unexpected => {
                    is_unary_operator = false;
                    
                    first_after_open = false;
                    if first_on_line {
                        first_on_line = false;
                        output_indent(&mut out_lines, expected_indent);
                    }
                    out_lines.last_mut().unwrap().extend(chunk);
                    
                },
            }
        };
        out_lines
    }
}

