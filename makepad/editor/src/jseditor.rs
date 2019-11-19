use render::*; 

use crate::textbuffer::*;
use crate::codeeditor::*;

#[derive(Clone)]
pub struct JSEditor {
    pub code_editor: CodeEditor,
}

impl JSEditor {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            code_editor: CodeEditor{
                folding_depth: 3,
                ..CodeEditor::proto(cx)
            }
        }
    }
    
    pub fn handle_js_editor(&mut self, cx: &mut Cx, event: &mut Event,  text_buffer: &mut TextBuffer) -> CodeEditorEvent {
        let ce = self.code_editor.handle_code_editor(cx, event, text_buffer);
        match ce {
            CodeEditorEvent::AutoFormat => {
                let formatted = JSTokenizer::auto_format(text_buffer).out_lines;
                self.code_editor.cursors.replace_lines_formatted(formatted, text_buffer);
                self.code_editor.view.redraw_view_area(cx);
            },
            _ => ()
        }
        ce
    }
    
    pub fn draw_js_editor(&mut self, cx: &mut Cx, text_buffer: &mut TextBuffer) {
        if text_buffer.needs_token_chunks() && text_buffer.lines.len() >0{
            let mut state = TokenizerState::new(&text_buffer.lines);
            let mut tokenizer = JSTokenizer::new();
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
        
        for (index, token_chunk) in text_buffer.token_chunks.iter_mut().enumerate(){
            self.code_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.messages.cursors);
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
        let start = chunk.len();
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
                    if (chunk.len()-start)>0 {
                        return TokenType::CommentChunk
                    }
                    
                    chunk.push(state.next);
                    state.advance();
                    return TokenType::Newline
                }
                else if state.next == ' ' {
                    if (chunk.len()-start)>0 {
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
                '0'..='9' => { // try to parse numbers
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
                'a'..='z' | 'A'..='Z' => { // try to parse keywords or identifiers
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
                if state.keyword(chunk, "r") {
                    if state.keyword(chunk, "y") {
                        return TokenType::Keyword
                    }
                    if state.keyword(chunk, "ue") {
                        return TokenType::Bool
                    }
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
    pub fn auto_format(text_buffer: &mut TextBuffer) -> FormatOutput {
        
        let extra_spacey = false;
        let pre_spacey = true;
        let mut out = FormatOutput::new();
        let mut tp = TokenParser::new(&text_buffer.flat_text, &text_buffer.token_chunks);
        
        struct ParenStack {
            expecting_newlines: bool,
            expected_indent: usize
        }
        
        let mut paren_stack: Vec<ParenStack> = Vec::new();
        
        paren_stack.push(ParenStack {
            expecting_newlines: true,
            expected_indent: 0,
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
                        && tp.prev_type() != TokenType::Operator
                        && tp.prev_type() != TokenType::Delimiter {
                        out.add_space();
                    }
                },
                TokenType::Newline => {
                    in_singleline_comment = false;
                    //paren_stack.last_mut().unwrap().angle_counter = 0;
                    if first_on_line && !in_singleline_comment && !in_multline_comment {
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
                },
                TokenType::Eof => {break},
                TokenType::ParenOpen => {
                    if first_on_line {
                        out.indent(expected_indent);
                    }
                    
                    paren_stack.push(ParenStack {
                        expecting_newlines: false,
                        expected_indent: expected_indent,
                    });
                    
                    first_after_open = true;
                    is_unary_operator = true;
                    
                    let is_curly = tp.cur_char() == '{';
                    if tp.cur_char() == '(' && (
                        tp.prev_type() == TokenType::Flow || tp.prev_type() == TokenType::Looping || tp.prev_type() == TokenType::Keyword
                    ) {
                        out.add_space();
                    }
                    if pre_spacey && is_curly && !first_on_line {
                        if tp.prev_char() != ' ' && tp.prev_char() != '{' && tp.prev_char() != '['
                            && tp.prev_char() != '(' && tp.prev_char() != ':' {
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
                    if paren_stack.last().unwrap().expecting_newlines == true
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
                    
                    if first_on_line {
                        first_on_line = false;
                        let extra_indent = if is_unary_operator {0}else {4};
                        out.indent(expected_indent + extra_indent);
                    }
                    
                    if (is_unary_operator && (tp.cur_char() == '-' || tp.cur_char() == '*' || tp.cur_char() == '&'))
                        || tp.cur_char() == '.' || tp.cur_char() == '!' {
                        out.extend(tp.cur_chunk());
                    }
                    else {
                        if tp.cur_char() == '?' {
                            out.strip_space();
                        }
                        else {
                            out.add_space();
                        }
                        out.extend(tp.cur_chunk());
                        if tp.next_type() != TokenType::Newline {
                            out.add_space();
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
                        out.indent(expected_indent);
                    }
                    out.extend(tp.cur_chunk());
                },
                // these are followeable by non unary operators
                TokenType::Identifier | TokenType::BuiltinType | TokenType::TypeName | TokenType::ThemeName |
                TokenType::Call | TokenType::String | TokenType::Regex | TokenType::Number |
                TokenType::Bool | TokenType::Unexpected | TokenType::Error | TokenType::Warning | TokenType::Defocus=> {
                    is_unary_operator = false;
                    
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

