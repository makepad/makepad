//use widgets::*;

#[derive(Clone, Default)]
pub struct TextBuffer{
    // Vec<Vec<char>> was chosen because, for all practical use (code) most lines are short
    // Concatenating the total into a utf8 string is trivial, and O(1) for windowing into the lines is handy.
    // Also inserting lines is pretty cheap even approaching 100k lines.
    // If you want to load a 100 meg single line file or something with >100k lines
    // other options are better. But these are not usecases for this editor.
    pub lines: Vec<Vec<char>>,
    pub load_id: u64,
    pub _char_count: usize
}

impl TextBuffer{

    pub fn offset_to_row_col(&self, char_offset:usize)->(usize,usize){
        let mut char_count = 0;
        for (row,line) in self.lines.iter().enumerate(){
            let next_char_count = char_count + line.len() + 1;
            if next_char_count > char_offset{
                return (row, char_offset - char_count)
            }
            char_count = next_char_count;
        }
        (0,0)
    }

    pub fn row_col_to_offset(&self, row:usize, col:usize)->usize{
        let mut char_count = 0;
        if row >= self.lines.len(){
            return self._char_count
        }
        for (ln_row, line) in self.lines.iter().enumerate(){
            if ln_row == row{
                return char_count + line.len().min(col);
            }
            char_count += line.len() + 1;
        }
        0
    }

    pub fn get_char_count(&self)->usize{
        self._char_count
    }

    pub fn get_line_count(&self)->usize{
        self.lines.len()
    }

    pub fn update_char_count(&mut self){
        let mut char_count = 0;
        for line in &self.lines{
            char_count += line.len()
        }
        char_count += self.lines.len() - 1; // invisible newline chars
        self._char_count = char_count;
    }
/*
    pub fn get_range(&mut self, start:usize, end:usize){
    }
    
    pub fn replace(&mut self, start:usize, end:usize, data:&str){
        self.update_char_count();
    }*/

    pub fn save_buffer(&mut self){
        //let out = self.lines.join("\n");
    }

    pub fn load_buffer(&mut self, data:&Vec<u8>){
        // alright we have to load it and split it on newlines
        if let Ok(utf8_data) = std::str::from_utf8(&data){
            self.lines = utf8_data.to_string().split("\n").map(|s| s.chars().collect()).collect();
            // lets be lazy and redraw all
        }
        self.update_char_count();
    }
}


pub struct TokenizerState<'a>{
    pub prev:char,
    pub cur:char,
    pub next:char,
    pub text_buffer:&'a TextBuffer,
    pub line_counter:usize,
    pub offset:usize,
    iter:std::slice::Iter<'a, char>
}

impl<'a> TokenizerState<'a>{
    pub fn new(text_buffer:&'a TextBuffer)->Self{
        let mut ret = Self{
            prev:'\0',
            text_buffer:text_buffer,
            line_counter:0,
            offset:0,
            cur:'\0',
            next:'\0',
            iter:text_buffer.lines[0].iter()
        };
        ret.advance_with_cur();
        ret
    }

    pub fn advance(&mut self){
        if let Some(next) = self.iter.next(){
            self.next = *next;
            self.offset += 1;
        }
        else{
            if self.line_counter < self.text_buffer.lines.len() - 1{
                self.line_counter += 1;
                self.offset += 1;
                self.iter = self.text_buffer.lines[self.line_counter].iter();
                self.next = '\n'
            }
            else{
                self.offset += 1;
                self.next = '\0'
            }
        }
    }

    pub fn next_is_digit(&self)->bool{
        self.next >= '0' && self.next <='9'
    }

    pub fn next_is_letter(&self)->bool{
        self.next >= 'a' && self.next <='z' || self.next >= 'A' && self.next <='Z'
    }

    pub fn next_is_lowercase_letter(&self)->bool{
        self.next >= 'a' && self.next <='z' 
    }

    pub fn next_is_uppercase_letter(&self)->bool{
        self.next >= 'A' && self.next <='Z' 
    }

    pub fn next_is_hex(&self)->bool{
        self.next >= '0' && self.next <='9' || self.next >= 'a' && self.next <= 'f' || self.next >= 'A' && self.next <='F'
    }

    pub fn advance_with_cur(&mut self){
        self.cur = self.next;
        self.advance();
    }

    pub fn advance_with_prev(&mut self){
        self.prev = self.cur;
        self.cur = self.next;
        self.advance();
    }

    pub fn keyword(&mut self, chunk:&mut Vec<char>, word:&str)->bool{
        for m in word.chars(){
            if m == self.next{ 
                chunk.push(m);
                self.advance();
            }
            else{
                return false
            }
        }
        return true
    }
}