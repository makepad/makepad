// the 'makepad rust' tokenizer

pub struct TokenizerState<'a> {
    pub prev: char,
    pub cur: char,
    pub next: char,
    pub lines: &'a Vec<Vec<char>>,
    pub line_start: usize,
    pub line_counter: usize,
    pub offset: usize,
    iter: std::slice::Iter<'a, char>
}

impl<'a> TokenizerState<'a> {
    pub fn new(lines: &'a Vec<Vec<char>>) -> Self {
        let mut ret = Self {
            lines: lines,
            line_start: 0,
            line_counter: 0,
            offset: 0,
            prev: '\0',
            cur: '\0',
            next: '\0',
            iter: lines[0].iter()
        };
        ret.advance_with_cur();
        ret
    }
    
    pub fn advance(&mut self) {
        if let Some(next) = self.iter.next() {
            self.next = *next;
            self.offset += 1;
        }
        else {
            self.next_line();
        }
    }
    
    pub fn next_line(&mut self) {
        if self.line_counter < self.lines.len() - 1 {
            self.line_counter += 1;
            self.line_start = self.offset;
            self.offset += 1;
            self.iter = self.lines[self.line_counter].iter();
            self.next = '\n'
        }
        else {
            self.offset += 1;
            self.next = '\0'
        }
    }
    
    pub fn next_is_digit(&self) -> bool {
        self.next >= '0' && self.next <= '9'
    }
    
    pub fn next_is_letter(&self) -> bool {
        self.next >= 'a' && self.next <= 'z'
            || self.next >= 'A' && self.next <= 'Z'
    }
    
    pub fn next_is_lowercase_letter(&self) -> bool {
        self.next >= 'a' && self.next <= 'z'
    }
    
    pub fn next_is_uppercase_letter(&self) -> bool {
        self.next >= 'A' && self.next <= 'Z'
    }
    
    pub fn next_is_hex(&self) -> bool {
        self.next >= '0' && self.next <= '9'
            || self.next >= 'a' && self.next <= 'f'
            || self.next >= 'A' && self.next <= 'F'
    }
    
    pub fn advance_with_cur(&mut self) {
        self.cur = self.next;
        self.advance();
    }
    
    pub fn advance_with_prev(&mut self) {
        self.prev = self.cur;
        self.cur = self.next;
        self.advance();
    }
    
    pub fn keyword(&mut self, chunk: &mut Vec<char>, word: &str) -> bool {
        for m in word.chars() {
            if m == self.next {
                chunk.push(m);
                self.advance();
            }
            else {
                return false
            }
        }
        return true
    }
}

#[derive(Clone)]
pub struct TokenChunk {
    pub token_type: TokenType,
    pub offset: usize,
    pub pair_token: usize,
    pub len: usize,
    pub next: char, 
    //    pub chunk: Vec<char>
}

impl TokenChunk {
    pub fn scan_last_token(token_chunks: &Vec<TokenChunk>) -> TokenType {
        let mut prev_tok_index = token_chunks.len();
        while prev_tok_index > 0 {
            let tt = &token_chunks[prev_tok_index - 1].token_type;
            if !tt.should_ignore() {
                return tt.clone();
            }
            prev_tok_index -= 1;
        }
        return TokenType::Unexpected
    }
    
    pub fn push_with_pairing(token_chunks: &mut Vec<TokenChunk>, pair_stack: &mut Vec<usize>, next: char, offset: usize, offset2: usize, token_type: TokenType)->bool {
        let mut invalid_pair = false;
        let pair_token = if token_type == TokenType::ParenOpen {
            pair_stack.push(token_chunks.len());
            token_chunks.len()
        }
        else if token_type == TokenType::ParenClose {
            if pair_stack.len() > 0 {
                let other = pair_stack.pop().unwrap();
                token_chunks[other].pair_token = token_chunks.len();
                other
            }
            else {
                invalid_pair = true;
                token_chunks.len()
            }
        }
        else {
            token_chunks.len()
        };
        token_chunks.push(TokenChunk {
            offset: offset,
            pair_token: pair_token,
            len: offset2 - offset,
            next: next,
            token_type: token_type.clone()
        });
        invalid_pair
    }
    
}


#[derive(Clone, PartialEq, Copy, Debug)]
pub enum TokenType {
    Whitespace,
    Newline,
    Keyword,
    Flow,
    Fn,
    TypeDef,
    Impl,
    Looping,
    Identifier,
    Call,
    Macro,
    TypeName,
    ThemeName,
    BuiltinType,
    Hash,
    
    Regex,
    String,
    Number,
    Bool,
    
    StringMultiBegin,
    StringChunk,
    StringMultiEnd,
    
    CommentLine,
    CommentMultiBegin,
    CommentChunk,
    CommentMultiEnd,
    
    ParenOpen,
    ParenClose,
    Operator,
    Namespace,
    Splat,
    Delimiter,
    Colon,
    
    Warning,
    Error,
    Defocus,
    
    Unexpected,
    Eof
}

impl TokenType {
    pub fn should_ignore(&self) -> bool {
        match self {
            TokenType::Whitespace => true,
            TokenType::Newline => true,
            TokenType::CommentLine => true,
            TokenType::CommentMultiBegin => true,
            TokenType::CommentChunk => true,
            TokenType::CommentMultiEnd => true,
            _ => false
        }
    }
}

