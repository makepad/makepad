 use render::*;

use crate::textcursor::*;

#[derive(Clone, Default)]
pub struct TextBuffer {
    // Vec<Vec<char>> was chosen because, for all practical use (code) most lines are short
    // Concatenating the total into a utf8 string is trivial, and O(1) for windowing into the lines is handy.
    // Also inserting lines is pretty cheap even approaching 100k lines.
    // If you want to load a 100 meg single line file or something with >100k lines
    // other options are better. But these are not usecases for this editor.
    pub lines: Vec<Vec<char>>,
    pub undo_stack: Vec<TextUndo>,
    pub redo_stack: Vec<TextUndo>,

    //pub load_file_read: FileRead,
    pub is_loading:bool,
    pub signal: Signal,

    pub mutation_id: u64,
    pub is_crlf: bool,
    pub messages: TextBufferMessages,
    pub flat_text: Vec<char>,
    pub token_chunks: Vec<TokenChunk>,
    pub token_chunks_id: u64,
    pub keyboard: TextBufferKeyboard,
} 

impl TextBuffer {
    pub fn needs_token_chunks(&mut self) -> bool {
        if self.token_chunks_id != self.mutation_id && !self.is_loading {
            self.token_chunks_id = self.mutation_id;
            self.token_chunks.truncate(0);
            self.flat_text.truncate(0);
            return true
        }
        return false
    }
}

pub const SIGNAL_TEXTBUFFER_LOADED: usize = 1;
pub const SIGNAL_TEXTBUFFER_MESSAGE_UPDATE: usize = 2;
pub const SIGNAL_TEXTBUFFER_JUMP_TO_OFFSET: usize = 3;
pub const SIGNAL_TEXTBUFFER_DATA_UPDATE: usize = 4;
pub const SIGNAL_TEXTBUFFER_KEYBOARD_UPDATE: usize = 5;

#[derive(Clone, Default)]
pub struct TextBufferKeyboard {
    pub modifiers: KeyModifiers,
    pub key_down: Option<KeyCode>,
    pub key_up: Option<KeyCode>
}

#[derive(Clone, Default)]
pub struct TextBufferMessages {
    pub gc_id: u64,
    // gc id for the update pass
    pub mutation_id: u64,
    // only if this matches the textbuffer mutation id are the messages valid
    pub cursors: Vec<TextCursor>,
    pub bodies: Vec<TextBufferMessage>,
    pub jump_to_offset: usize
}

#[derive(Clone, PartialEq)]
pub enum TextBufferMessageLevel {
    Error,
    Warning,
    Log
}

#[derive(Clone)]
pub struct TextBufferMessage {
    pub level: TextBufferMessageLevel,
    pub body: String
}

/*
impl TextBuffers {
    pub fn from_path(&mut self, cx: &mut Cx, path: &str) -> &mut TextBuffer {
        let root_path = &self.root_path;
        self.storage.entry(path.to_string()).or_insert_with( || {
            TextBuffer {
                signal: cx.new_signal(),
                mutation_id: 1,
                load_file_read: cx.file_read(&format!("{}{}", root_path, path)),
                ..Default::default()
            }
        })
    }
    
    pub fn save_file(&mut self, cx: &mut Cx, path: &str) {
        let text_buffer = self.storage.get(path);
        if let Some(text_buffer) = text_buffer {
            let string = text_buffer.get_as_string();
            cx.file_write(&format!("{}{}", self.root_path, path), string.as_bytes());
            //cx.http_send("POST", path, "192.168.0.20", "2001", &string);
        }
    }
    
    pub fn handle_file_read(&mut self, cx: &mut Cx, fr: &FileReadEvent) -> bool {
        for (_path, text_buffer) in &mut self.storage {
            if let Some(utf8_data) = text_buffer.load_file_read.resolve_utf8(fr) {
                if let Ok(utf8_data) = utf8_data {
                    // TODO HANDLE ERROR CASE
                    text_buffer.is_crlf = !utf8_data.find("\r\n").is_none();
                    text_buffer.lines = TextBuffer::split_string_to_lines(&utf8_data.to_string());
                    cx.send_signal(text_buffer.signal, SIGNAL_TEXTBUFFER_LOADED);
                }
                return true
            }
        }
        return false;
    }
}
*/
#[derive(Clone, Copy)]
pub struct TextPos {
    pub row: usize,
    pub col: usize
}

impl TextPos {
    pub fn dist(&self, other: &TextPos) -> f64 {
        let dr = (self.row as f64) - (other.row as f64);
        let dc = (self.col as f64) - (other.col as f64);
        (dr * dr + dc * dc).sqrt()
    }
    
    pub fn zero() -> TextPos {
        TextPos {row: 0, col: 0}
    }
}

#[derive(Clone, PartialEq)]
pub enum TextUndoGrouping {
    Space,
    Newline,
    Character(u64),
    Backspace,
    Delete(usize),
    Block,
    Tab,
    Cut,
    Format,
    Other
}

impl Default for TextUndoGrouping {
    fn default() -> TextUndoGrouping {
        TextUndoGrouping::Other
    }
}

impl TextUndoGrouping {
    fn wants_grouping(&self) -> bool {
        match self {
            TextUndoGrouping::Space => true,
            TextUndoGrouping::Newline => false,
            TextUndoGrouping::Character(_) => true,
            TextUndoGrouping::Backspace => true,
            TextUndoGrouping::Delete(_) => true,
            TextUndoGrouping::Block => false,
            TextUndoGrouping::Tab => false,
            TextUndoGrouping::Format => false,
            TextUndoGrouping::Cut => false,
            TextUndoGrouping::Other => false
        }
    }
}

#[derive(Clone)]
pub struct TextUndo {
    pub ops: Vec<TextOp>,
    pub grouping: TextUndoGrouping,
    pub cursors: TextCursorSet
}

#[derive(Clone)]
pub struct TextOp {
    pub start: usize,
    pub len: usize,
    pub lines: Vec<Vec<char>>,
}

fn calc_char_count(lines: &Vec<Vec<char>>) -> usize {
    let mut char_count = 0;
    for line in lines {
        char_count += line.len()
    }
    char_count += lines.len() - 1;
    // invisible newline chars
    char_count
}

impl TextBuffer {
    
    pub fn offset_to_text_pos(&self, char_offset: usize) -> TextPos {
        let mut char_count = 0;
        for (row, line) in self.lines.iter().enumerate() {
            let next_char_count = char_count + line.len() + 1;
            if next_char_count > char_offset {
                return TextPos {row: row, col: char_offset - char_count}
            }
            char_count = next_char_count;
        }
        TextPos {row: self.lines.len().max(1) - 1, col: 0}
    }
    
    pub fn offset_to_text_pos_next(&self, query_off: usize, old_pos: TextPos, old_off: usize) -> TextPos {
        let mut row = old_pos.row;
        let mut iter_off = old_off - old_pos.col;
        while row < self.lines.len() {
            let line = &self.lines[row];
            let next_off = iter_off + line.len() + 1;
            if next_off > query_off {
                return TextPos {row: row, col: query_off - iter_off}
            }
            iter_off = next_off;
            row += 1;
        }
        TextPos {row: self.lines.len().max(1) - 1, col: 0}
    }
    
    pub fn text_pos_to_offset(&self, pos: TextPos) -> usize {
        let mut char_count = 0;
        if pos.row >= self.lines.len() {
            return self.calc_char_count()
        }
        for (ln_row, line) in self.lines.iter().enumerate() {
            if ln_row == pos.row {
                return char_count + (line.len()).min(pos.col);
            }
            char_count += line.len() + 1;
        }
        0
    }
    
    pub fn get_nearest_line_range(&self, offset: usize) -> (usize, usize) {
        let pos = self.offset_to_text_pos(offset);
        let line = &self.lines[pos.row];
        return (offset - pos.col, line.len() + if pos.row < (line.len().max(1) - 1) {1}else {0})
    }
    
    pub fn calc_next_line_indent_depth(&self, offset: usize, tabsize: usize) -> (usize, usize) {
        let pos = self.offset_to_text_pos(offset);
        let line = &self.lines[pos.row];
        let mut prev_index = pos.col;
        if prev_index == 0 || prev_index > line.len() {
            return (offset - pos.col, 0);
        };
        
        let mut instep = 0;
        while prev_index > 0 {
            let prev = line[prev_index - 1];
            if prev == ')' || prev == '}' || prev == ']' {
                break;
            }
            if prev == '{' || prev == '(' || prev == '[' {
                instep = tabsize;
                break;
            }
            prev_index -= 1;
        }
        for (i, ch) in line.iter().enumerate() {
            if *ch != ' ' {
                return (offset - pos.col, i + instep);
            }
        };
        return (offset - pos.col, line.len());
    }
    
    pub fn calc_line_indent_depth(&self, row: usize) -> usize {
        let line = &self.lines[row];
        for (i, ch) in line.iter().enumerate() {
            if *ch != ' ' {
                return i
            }
        };
        return line.len()
    }
    
    pub fn calc_backspace_line_indent_depth_and_pair(&self, offset: usize) -> (usize, usize) {
        let pos = self.offset_to_text_pos(offset);
        let line = &self.lines[pos.row];
        for i in 0..line.len() {
            let ch = line[i];
            if ch != ' ' {
                if i == pos.col {
                    return (offset - (i + 1), 1 + i);
                }
                // check pair removal
                if pos.col >= 1 && pos.col <line.len() {
                    let pch = line[pos.col - 1];
                    let nch = line[pos.col];
                    if pch == '{' && nch == '}'
                        || pch == '(' && nch == ')'
                        || pch == '[' && nch == ']' {
                        return (offset - 1, 2)
                    }
                }
                return (offset - 1, 1);
            }
        };
        return ((offset - pos.col - 1), line.len() + 1);
    }
    
    pub fn calc_deletion_whitespace(&self, offset: usize) -> Option<(usize, usize, usize, usize)> {
        let pos = self.offset_to_text_pos(offset);
        if self.lines.len() < 1 || pos.row >= self.lines.len() - 1 {
            return None
        }
        let line1 = &self.lines[pos.row];
        let mut line1_ws = 0;
        for ch in line1 {
            if *ch != ' ' {
                break;
            }
            line1_ws += 1;
        };
        
        let line2 = &self.lines[pos.row + 1];
        let mut line2_ws = 0;
        for ch in line2 {
            if *ch != ' ' {
                break;
            }
            line2_ws += 1;
        };
        
        return Some((offset - pos.col, line1_ws, line1.len(), line2_ws));
    }
    
    
    pub fn calc_deindent_whitespace(&self, offset: usize) -> Option<(usize, usize, usize)> {
        let pos = self.offset_to_text_pos(offset);
        if self.lines.len() < 1 || pos.row >= self.lines.len() {
            return None
        }
        let line1 = &self.lines[pos.row];
        let mut line1_ws = 0;
        for ch in line1 {
            if *ch != ' ' {
                break;
            }
            line1_ws += 1;
        };
        
        return Some((offset - pos.col, line1_ws, line1.len()));
    }
    
    pub fn calc_char_count(&self) -> usize {
        calc_char_count(&self.lines)
    }
    
    pub fn get_line_count(&self) -> usize {
        self.lines.len()
    }
    
    pub fn get_range_as_string(&self, start: usize, len: usize, ret: &mut String) {
        let mut pos = self.offset_to_text_pos(start);
        for _ in 0..len {
            let line = &self.lines[pos.row];
            if pos.col >= line.len() {
                ret.push('\n');
                pos.col = 0;
                pos.row += 1;
                if pos.row >= self.lines.len() {
                    return;
                }
            }
            else {
                ret.push(line[pos.col]);
                pos.col += 1;
            }
        };
    }
    
    
    pub fn get_char(&self, start: usize) -> char {
        let pos = self.offset_to_text_pos(start);
        let line = &self.lines[pos.row];
        if pos.row == self.lines.len() - 1 && pos.col >= line.len() {
            return '\0'
        }
        if pos.col >= line.len() {
            return '\n'
        }
        return line[pos.col]
    }
    
    pub fn get_as_string(&self) -> String {
        let mut ret = String::new();
        for i in 0..self.lines.len() {
            let line = &self.lines[i];
            for ch in line {
                ret.push(*ch);
            }
            if i != self.lines.len() - 1 {
                if self.is_crlf{
                    ret.push('\r');
                    ret.push('\n');
                }
                else{
                    ret.push('\n');
                }
            }
        }
        return ret
    }
    
    pub fn load_from_utf8(&mut self, cx:&mut Cx, utf8:&str){
        self.is_loading = false;
        self.is_crlf =  !utf8.find("\r\n").is_none();
        self.lines = TextBuffer::split_string_to_lines(utf8);
        self.mutation_id += 1;
        cx.send_signal(self.signal, SIGNAL_TEXTBUFFER_LOADED);
    }
    
    pub fn replace_line(&mut self, row: usize, start_col: usize, len: usize, rep_line: Vec<char>) -> Vec<char> {
        self.mutation_id += 1;
        self.lines[row].splice(start_col..(start_col + len), rep_line).collect()
    }
    
    pub fn copy_line(&self, row: usize, start_col: usize, len: usize) -> Vec<char> {
        let line = &self.lines[row];
        if start_col >= line.len() {
            return vec![]
        }
        if start_col + len > line.len() {
            self.lines[row][start_col..line.len()].iter().cloned().collect()
        }
        else {
            self.lines[row][start_col..(start_col + len)].iter().cloned().collect()
        }
    }
    
    pub fn replace_range(&mut self, start: usize, len: usize, mut rep_lines: Vec<Vec<char>>) -> Vec<Vec<char>> {
        self.mutation_id += 1;
        let start_pos = self.offset_to_text_pos(start);
        let end_pos = self.offset_to_text_pos_next(start + len, start_pos, start);
        
        if start_pos.row == end_pos.row && rep_lines.len() == 1 { // replace in one line
            let rep_line_zero = rep_lines.drain(0..1).next().unwrap();
            
            if start_pos.col>end_pos.col{ 
               return vec![];
            }
            let line = self.lines[start_pos.row].splice(start_pos.col..end_pos.col, rep_line_zero).collect();
            return vec![line];
        }
        else {
            if rep_lines.len() == 1 { // we are replacing multiple lines with one line
                // drain first line
                let rep_line_zero = rep_lines.drain(0..1).next().unwrap();
                
                // replace it in the first
                let first = self.lines[start_pos.row].splice(start_pos.col.., rep_line_zero).collect();
                
                // collect the middle ones
                let mut middle: Vec<Vec<char>> = self.lines.drain((start_pos.row + 1)..(end_pos.row)).collect();
                
                // cut out the last bit
                let last: Vec<char> = self.lines[start_pos.row + 1].drain(0..end_pos.col).collect();
                
                // last line bit
                let mut last_line = self.lines.drain((start_pos.row + 1)..(start_pos.row + 2)).next().unwrap();
                
                // merge start_row+1 into start_row
                self.lines[start_pos.row].append(&mut last_line);
                
                // concat it all together
                middle.insert(0, first);
                middle.push(last);
                
                return middle
            }
            else if start_pos.row == end_pos.row { // replacing single line with multiple lines
                let mut last_bit: Vec<char> = self.lines[start_pos.row].drain(end_pos.col..).collect();
                // but we have co drain end_col..
                
                // replaced first line
                let rep_lines_len = rep_lines.len();
                let rep_line_first: Vec<char> = rep_lines.drain(0..1).next().unwrap();
                let line = self.lines[start_pos.row].splice(start_pos.col.., rep_line_first).collect();
                
                // splice in middle rest
                let rep_line_mid = rep_lines.drain(0..(rep_lines.len()));
                self.lines.splice((start_pos.row + 1)..(start_pos.row + 1), rep_line_mid);
                
                // append last bit
                self.lines[start_pos.row + rep_lines_len - 1].append(&mut last_bit);
                
                return vec![line];
            }
            else { // replaceing multiple lines with multiple lines
                // drain and replace last line
                let rep_line_last = rep_lines.drain((rep_lines.len() - 1)..(rep_lines.len())).next().unwrap();
                let last = self.lines[end_pos.row].splice(..end_pos.col, rep_line_last).collect();
                
                // swap out middle lines and drain them
                let rep_line_mid = rep_lines.drain(1..(rep_lines.len()));
                let mut middle: Vec<Vec<char>> = self.lines.splice((start_pos.row + 1)..end_pos.row, rep_line_mid).collect();
                
                // drain and replace first line
                let rep_line_zero = rep_lines.drain(0..1).next().unwrap();
                let first = self.lines[start_pos.row].splice(start_pos.col.., rep_line_zero).collect();
                
                // concat it all together
                middle.insert(0, first);
                middle.push(last);
                return middle
            }
        }
    }
    
    pub fn replace_lines(&mut self, start_row: usize, end_row: usize, rep_lines: Vec<Vec<char>>) -> TextOp {
        let start = self.text_pos_to_offset(TextPos {row: start_row, col: 0});
        let end = self.text_pos_to_offset(TextPos {row: end_row, col: 0});
        let end_mark = if end_row >= self.lines.len() {0}else {1};
        let rep_lines_chars = calc_char_count(&rep_lines);
        let lines = self.replace_range(start, end - start - end_mark, rep_lines);
        TextOp {
            start: start,
            len: rep_lines_chars,
            lines: lines
        }
    }
    
    pub fn split_string_to_lines(string: &str) -> Vec<Vec<char>> {
        if !string.find("\r\n").is_none(){
            return string.split("\r\n").map( | s | s.chars().collect()).collect()
        }
        else{
            return string.split("\n").map( | s | s.chars().collect()).collect()
        }
    }
    
    pub fn replace_lines_with_string(&mut self, start: usize, len: usize, string: &str) -> TextOp {
        let rep_lines = Self::split_string_to_lines(string);
        let rep_lines_chars = calc_char_count(&rep_lines);
        let lines = self.replace_range(start, len, rep_lines);
        TextOp {
            start: start,
            len: rep_lines_chars,
            lines: lines
        }
    }
    
    pub fn replace_line_with_string(&mut self, start: usize, row: usize, col: usize, len: usize, string: &str) -> TextOp {
        let rep_line: Vec<char> = string.chars().collect();
        let rep_line_chars = rep_line.len();
        let line = self.replace_line(row, col, len, rep_line);
        TextOp {
            start: start,
            len: rep_line_chars,
            lines: vec![line]
        }
    }
    
    pub fn replace_with_textop(&mut self, text_op: TextOp) -> TextOp {
        let rep_lines_chars = calc_char_count(&text_op.lines);
        let lines = self.replace_range(text_op.start, text_op.len, text_op.lines);
        TextOp {
            start: text_op.start,
            len: rep_lines_chars,
            lines: lines
        }
    }
    
    pub fn save_buffer(&mut self) {
        //let out = self.lines.join("\n");
    }
    
    pub fn undoredo(&mut self, mut text_undo: TextUndo, cursor_set: &mut TextCursorSet) -> TextUndo {
        let mut ops = Vec::new();
        while text_undo.ops.len() > 0 {
            let op = text_undo.ops.pop().unwrap();
            //text_undo.ops.len() - 1);
            ops.push(self.replace_with_textop(op));
        }
        let text_undo_inverse = TextUndo {
            ops: ops,
            grouping: text_undo.grouping,
            cursors: cursor_set.clone()
        };
        cursor_set.set = text_undo.cursors.set.clone();
        cursor_set.last_cursor = text_undo.cursors.last_cursor;
        text_undo_inverse
    }
    
    // todo make more reuse in these functions
    pub fn undo(&mut self, grouped: bool, cursor_set: &mut TextCursorSet) {
        
        if self.undo_stack.len() == 0 {
            return;
        }
        let mut last_grouping = TextUndoGrouping::Other;
        let mut first = true;
        while self.undo_stack.len() > 0 {
            if !first && !grouped {
                break
            }
            if self.undo_stack.last().unwrap().grouping != last_grouping && !first {
                break
            }
            first = false;
            let text_undo = self.undo_stack.pop().unwrap();
            let wants_grouping = text_undo.grouping.wants_grouping();
            last_grouping = text_undo.grouping.clone();
            let text_redo = self.undoredo(text_undo, cursor_set);
            self.redo_stack.push(text_redo);
            if !wants_grouping {
                break;
            }
        }
    }
    
    pub fn redo(&mut self, grouped: bool, cursor_set: &mut TextCursorSet) {
        if self.redo_stack.len() == 0 {
            return;
        }
        let mut last_grouping = TextUndoGrouping::Other;
        let mut first = true;
        while self.redo_stack.len() > 0 {
            if !first {
                if self.redo_stack.last().unwrap().grouping != last_grouping || !grouped {
                    break
                }
            }
            first = false;
            let text_redo = self.redo_stack.pop().unwrap();
            let wants_grouping = text_redo.grouping.wants_grouping();
            last_grouping = text_redo.grouping.clone();
            let text_undo = self.undoredo(text_redo, cursor_set);
            self.undo_stack.push(text_undo);
            if !wants_grouping {
                break;
            }
        }
    }
    
}

pub struct LineTokenizer<'a> {
    pub prev: char,
    pub cur: char,
    pub next: char,
    iter: std::str::Chars<'a>
}

impl<'a> LineTokenizer<'a> {
    pub fn new(st: &'a str) -> Self {
        let mut ret = Self {
            prev: '\0',
            cur: '\0',
            next: '\0',
            iter: st.chars()
        };
        ret.advance();
        ret
    }
    
    pub fn advance(&mut self) {
        if let Some(next) = self.iter.next() {
            self.next = next;
        }
        else {
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

pub struct TokenizerState<'a> {
    pub prev: char,
    pub cur: char,
    pub next: char,
    pub lines: &'a Vec<Vec<char>>,
    pub line_counter: usize,
    pub offset: usize,
    iter: std::slice::Iter<'a, char>
}

impl<'a> TokenizerState<'a> {
    pub fn new(lines: &'a Vec<Vec<char>>) -> Self {
        let mut ret = Self {
            lines: lines,
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

#[derive(Clone, PartialEq, Copy, Debug)]
pub enum TokenType {
    Whitespace,
    Newline,
    Keyword,
    Flow,
    Fn,
    TypeDef,
    Looping,
    Identifier,
    Call,
    TypeName,
    ThemeName,
    BuiltinType,
    Hash,
    
    Regex,
    String,
    Number,
    Bool,
    
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
    
    pub fn push_with_pairing(token_chunks: &mut Vec<TokenChunk>, pair_stack: &mut Vec<usize>, next: char, offset: usize, offset2: usize, token_type: TokenType) {
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
        })
    }
    
}

pub struct TokenParserItem {
    pub chunk: Vec<char>,
    pub token_type: TokenType,
}

pub struct TokenParser<'a> {
    pub tokens: &'a Vec<TokenChunk>,
    pub flat_text: &'a Vec<char>,
    pub index: usize,
    pub next_index: usize
}

impl <'a>TokenParser<'a> {
    pub fn new(flat_text: &'a Vec<char>, token_chunks: &'a Vec<TokenChunk>) -> TokenParser<'a> {
        TokenParser {
            tokens: token_chunks,
            flat_text: flat_text,
            index: 0,
            next_index: 0
        }
    }
    
    pub fn advance(&mut self) -> bool {
        if self.next_index >= self.tokens.len() {
            return false
        }
        self.index = self.next_index;
        self.next_index += 1;
        return true;
    }
    
    pub fn prev_type(&self) -> TokenType {
        if self.index > 0 {
            self.tokens[self.index - 1].token_type
        }
        else {
            TokenType::Unexpected
        }
    }
    
    pub fn cur_type(&self) -> TokenType {
        self.tokens[self.index].token_type
    }
    
    pub fn next_type(&self) -> TokenType {
        if self.index < self.tokens.len() - 1 {
            self.tokens[self.index + 1].token_type
        }
        else {
            TokenType::Unexpected
        }
    }
    
    pub fn prev_char(&self) -> char {
        if self.index > 0 {
            let len = self.tokens[self.index - 1].len;
            let ch = self.flat_text[self.tokens[self.index - 1].offset];
            if len == 1 || ch == ' ' {
                return ch
            }
        }
        '\0'
    }
    
    pub fn cur_char(&self) -> char {
        let len = self.tokens[self.index].len;
        let ch = self.flat_text[self.tokens[self.index].offset];
        if len == 1 || ch == ' ' {
            return ch
        }
        '\0'
    }
    
    pub fn cur_chunk(&self) -> &[char] {
        let offset = self.tokens[self.index].offset;
        let len = self.tokens[self.index].len;
        &self.flat_text[offset..(offset + len)]
    }
    
    pub fn next_char(&self) -> char {
        if self.index < self.tokens.len() - 1 {
            let len = self.tokens[self.index + 1].len;
            let ch = self.flat_text[self.tokens[self.index + 1].offset];
            if len == 1 || ch == ' ' {
                return ch
            }
        }
        '\0'
    }
}

pub struct FormatOutput {
    pub out_lines: Vec<Vec<char >>
}

impl FormatOutput {
    pub fn new() -> FormatOutput {
        FormatOutput {
            out_lines: Vec::new()
        }
    }
    
    pub fn indent(&mut self, indent_depth: usize) {
        let last_line = self.out_lines.last_mut().unwrap();
        for _ in 0..indent_depth {
            last_line.push(' ');
        }
    }
    
    pub fn strip_space(&mut self) {
        let last_line = self.out_lines.last_mut().unwrap();
        if last_line.len()>0 && *last_line.last().unwrap() == ' ' {
            last_line.pop();
        }
    }
    
    pub fn new_line(&mut self) {
        self.out_lines.push(Vec::new());
    }
    
    pub fn extend(&mut self, chunk: &[char]) {
        let last_line = self.out_lines.last_mut().unwrap();
        last_line.extend_from_slice(chunk);
    }
    
    pub fn add_space(&mut self) {
        let last_line = self.out_lines.last_mut().unwrap();
        if last_line.len()>0 {
            if *last_line.last().unwrap() != ' ' {
                last_line.push(' ');
            }
        }
        else {
            last_line.push(' ');
        }
    }
    
}
