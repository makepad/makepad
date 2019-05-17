use widget::*;
use crate::textcursor::*;
use std::collections::HashMap;

#[derive(Clone, Default)]
pub struct TextBuffer{
    // Vec<Vec<char>> was chosen because, for all practical use (code) most lines are short
    // Concatenating the total into a utf8 string is trivial, and O(1) for windowing into the lines is handy.
    // Also inserting lines is pretty cheap even approaching 100k lines.
    // If you want to load a 100 meg single line file or something with >100k lines
    // other options are better. But these are not usecases for this editor.
    pub lines: Vec<Vec<char>>,
    pub undo_stack: Vec<TextUndo>,
    pub redo_stack: Vec<TextUndo>,
    pub load_id: u64,
    pub signal_id: u64,
    pub mutation_id: u64,
    pub messages: TextBufferMessages,
}

pub const SIGNAL_TEXTBUFFER_MESSAGE_UPDATE:u64 = 1;
pub const SIGNAL_TEXTBUFFER_JUMP_TO_OFFSET:u64 = 2;

#[derive(Clone, Default)]
pub struct TextBufferMessages{
    pub gc_id:u64, // gc id for the update pass
    pub mutation_id:u64, // only if this matches the textbuffer mutation id are the messages valid
    pub cursors:Vec<TextCursor>,
    pub bodies:Vec<TextBufferMessage>,
    pub jump_to_offset:usize
}

#[derive(Clone)]
pub enum TextBufferMessageLevel{
    Error,
    Warning,
}

#[derive(Clone)]
pub struct TextBufferMessage{
    pub level:TextBufferMessageLevel,
    pub body:String
}

pub struct TextBuffers{
    pub root_path: String,
    pub storage: HashMap<String, TextBuffer>
}

impl TextBuffers{
    pub fn from_path(&mut self, cx:&mut Cx, path:&str)->&mut TextBuffer{
        let root_path = &self.root_path;
        self.storage.entry(path.to_string()).or_insert_with(||{
            TextBuffer{
                signal_id:cx.new_signal_id(),
                load_id:cx.read_file(&format!("{}{}",root_path, path)),
                ..Default::default()
            }
        })
    }

    pub fn save_file(&mut self, cx:&mut Cx, path:&str){
        let text_buffer = self.storage.get(path);
        if let Some(text_buffer) = text_buffer{
            let data = text_buffer.get_as_string().bytes().collect();
            cx.write_file(&format!("{}{}",self.root_path,path),data);
        }
    }

    pub fn handle_file_read(&mut self, fr:&FileReadEvent)->bool{
        for (_path, text_buffer) in &mut self.storage{
            if text_buffer.load_id == fr.read_id{
                text_buffer.load_id = 0;
                if let Ok(str_data) = &fr.data{
                    text_buffer.load_buffer(str_data);
                    return true;
                }
            }
        }
        return false
    }
}


#[derive(Clone, Copy)]
pub struct TextPos{
    pub row:usize,
    pub col:usize
}

impl TextPos{
    pub fn dist(&self, other:&TextPos)->f64{
        let dr = (self.row as f64) - (other.row as f64);
        let dc = (self.col as f64) - (other.col as f64);
        (dr*dr+dc*dc).sqrt()
    }

    pub fn zero()->TextPos{
        TextPos{row:0, col:0}
    }
}

#[derive(Clone,PartialEq)]
pub enum TextUndoGrouping{
    Space,
    Newline,
    Character,
    Backspace,
    Delete(usize),
    Block,
    Tab,
    Cut,
    Other
}

impl Default for TextUndoGrouping{
    fn default()->TextUndoGrouping{
        TextUndoGrouping::Other
    }
}

impl TextUndoGrouping{
    fn wants_grouping(&self)->bool{
        match self{
            TextUndoGrouping::Space=>true,
            TextUndoGrouping::Newline=>false,
            TextUndoGrouping::Character=>true,
            TextUndoGrouping::Backspace=>true,
            TextUndoGrouping::Delete(_)=>true,
            TextUndoGrouping::Block=>false,
            TextUndoGrouping::Tab=>false,
            TextUndoGrouping::Cut=>false,
            TextUndoGrouping::Other=>false
        }
    }
}

#[derive(Clone)]
pub struct TextUndo{
    pub ops:Vec<TextOp>,
    pub grouping:TextUndoGrouping,
    pub cursors:TextCursorSet
}

#[derive(Clone)]
pub struct TextOp{
    pub start:usize,
    pub len:usize,
    pub lines:Vec<Vec<char>>,
}

fn calc_char_count(lines:&Vec<Vec<char>>)->usize{
    let mut char_count = 0;
    for line in lines{
        char_count += line.len()
    }
    char_count += lines.len() - 1; // invisible newline chars
    char_count
}

impl TextBuffer{

    pub fn offset_to_text_pos(&self, char_offset:usize)->TextPos{
        let mut char_count = 0;
        for (row,line) in self.lines.iter().enumerate(){
            let next_char_count = char_count + line.len() + 1;
            if next_char_count > char_offset{
                return TextPos{row:row, col:char_offset - char_count}
            }
            char_count = next_char_count;
        }
        TextPos{row:0, col:0}
    }

    pub fn offset_to_text_pos_next(&self, query_off:usize, old_pos:TextPos, old_off:usize)->TextPos{
        let mut row = old_pos.row;
        let mut iter_off = old_off - old_pos.col;
        while row < self.lines.len(){
            let line = &self.lines[row];
            let next_off = iter_off + line.len() + 1;
            if next_off > query_off{
                return TextPos{row:row, col:query_off - iter_off}
            }
            iter_off = next_off;
            row += 1;
        }
        TextPos{row:0, col:0}
    }

    pub fn text_pos_to_offset(&self, pos:TextPos)->usize{
        let mut char_count = 0;
        if pos.row >= self.lines.len(){
            return self.calc_char_count()
        }
        for (ln_row, line) in self.lines.iter().enumerate(){
            if ln_row == pos.row{
                return char_count + line.len().min(pos.col);
            }
            char_count += line.len() + 1;
        }
        0
    }

    pub fn get_nearest_line_range(&self, offset:usize)->(usize, usize){
        let pos = self.offset_to_text_pos(offset);
        let line = &self.lines[pos.row];
        return (offset - pos.col, line.len() + if pos.row < line.len()-1{1}else{0})
    }

    pub fn calc_next_line_indent_depth(&self, offset:usize, tabsize:usize)->usize{
        let pos = self.offset_to_text_pos(offset);
        let line = &self.lines[pos.row];
        let prev_index = pos.col;
        if prev_index == 0 || prev_index > line.len(){
            return 0;
        };
        let prev = line[prev_index-1];
        let instep = if prev == '{' || prev == '(' || prev == '['{tabsize}else{0};
        for (i,ch) in line.iter().enumerate(){
            if *ch != ' '{
                return i + instep;
            }
        };
        return line.len();
    }

    pub fn calc_line_indent_depth(&self, row:usize)->usize{
        let line = &self.lines[row];
        for (i,ch) in line.iter().enumerate(){
            if *ch != ' '{
                return i
            }
        };
        return line.len()
    }

    pub fn calc_backspace_line_indent_depth_and_pair(&self, offset:usize)->(usize, usize){
        let pos = self.offset_to_text_pos(offset);
        let line = &self.lines[pos.row];
        for i in 0..line.len(){
            let ch = line[i];
            if ch != ' '{
                if i == pos.col{
                    
                    return (offset-(i+1), 1 + i);
                }
                // check pair removal
                if pos.col >= 1 && pos.col <line.len(){
                    let pch = line[pos.col-1];
                    let nch = line[pos.col];
                    if pch == '{' && nch == '}' || pch == '(' && nch == ')' || pch == '[' && nch == ']'{
                        return (offset - 1, 2)
                    }
                }
                return (offset - 1, 1);
            }
        };
        return ((offset - pos.col - 1), line.len()+1);
    }

    pub fn calc_delete_line_indent_depth(&self, offset:usize)->usize{
        let pos = self.offset_to_text_pos(offset);
        if self.lines.len() < 1 || pos.col != self.lines[pos.row].len() || pos.row >= self.lines.len() - 1{
            return 0
        }
        let line = &self.lines[pos.row+1];
        for (i,ch) in line.iter().enumerate(){
            if *ch != ' '{
                return i;
            }
        };
        return line.len();
    }

    pub fn calc_char_count(&self)->usize{
        calc_char_count(&self.lines)
    }

    pub fn get_line_count(&self)->usize{
        self.lines.len()
    }

    pub fn get_range_as_string(&self, start:usize, len:usize, ret:&mut String){
        let mut pos = self.offset_to_text_pos(start);
        for _ in 0..len{
            let line = &self.lines[pos.row];
            if pos.col >= line.len(){
                ret.push('\n');
                pos.col = 0;
                pos.row += 1;
                if pos.row >= self.lines.len(){
                    return;
                }
            }
            else{
                ret.push(line[pos.col]);
                pos.col += 1;
            }
        };
    }

    pub fn get_as_string(&self)->String{
        let mut ret = String::new();
        for i in 0..self.lines.len(){
            let line = &self.lines[i];
            for ch in line{
                ret.push(*ch);
            }
            if i != self.lines.len()-1{
                ret.push('\n');
            }
        }
        return ret
    }

    pub fn replace_line(&mut self, row:usize, start_col:usize, len:usize, rep_line:Vec<char>)->Vec<char>{
        self.mutation_id += 1;
        self.lines[row].splice(start_col..(start_col+len), rep_line).collect()
    }

    pub fn copy_line(&self, row:usize, start_col:usize, len:usize)->Vec<char>{
        let line = &self.lines[row];
        if start_col + len > line.len(){
            self.lines[row][start_col..line.len()].iter().cloned().collect()
        }
        else{
            self.lines[row][start_col..(start_col+len)].iter().cloned().collect()
        }
    }

    pub fn replace_range(&mut self, start:usize, len:usize, mut rep_lines:Vec<Vec<char>>)->Vec<Vec<char>>{
        self.mutation_id += 1;
        let start_pos = self.offset_to_text_pos(start);
        let end_pos = self.offset_to_text_pos_next(start+len,start_pos, start);
        
        if start_pos.row == end_pos.row && rep_lines.len() == 1{ // replace in one line
            let rep_line_zero = rep_lines.drain(0..1).next().unwrap();
            let line = self.lines[start_pos.row].splice(start_pos.col..end_pos.col, rep_line_zero).collect();
            return vec![line];
        }
        else{
            if rep_lines.len() == 1{ // we are replacing multiple lines with one line
                // drain first line
                let rep_line_zero = rep_lines.drain(0..1).next().unwrap();

                // replace it in the first
                let first = self.lines[start_pos.row].splice(start_pos.col.., rep_line_zero).collect();

                // collect the middle ones
                let mut middle:Vec<Vec<char>> = self.lines.drain((start_pos.row+1)..(end_pos.row)).collect();

                // cut out the last bit
                let last:Vec<char> = self.lines[start_pos.row+1].drain(0..end_pos.col).collect();
                
                // last line bit
                let mut last_line = self.lines.drain((start_pos.row+1)..(start_pos.row+2)).next().unwrap();

                // merge start_row+1 into start_row
                self.lines[start_pos.row].append(&mut last_line);

                // concat it all together
                middle.insert(0, first);
                middle.push(last);

                return middle
            }
            else if start_pos.row == end_pos.row{ // replacing single line with multiple lines
                let mut last_bit:Vec<char> =  self.lines[start_pos.row].drain(end_pos.col..).collect();// but we have co drain end_col..

                // replaced first line
                let rep_lines_len = rep_lines.len();
                let rep_line_first:Vec<char> = rep_lines.drain(0..1).next().unwrap();
                let line = self.lines[start_pos.row].splice(start_pos.col.., rep_line_first).collect();

                // splice in middle rest
                let rep_line_mid = rep_lines.drain(0..(rep_lines.len()));
                self.lines.splice( (start_pos.row+1)..(start_pos.row+1), rep_line_mid);
                
                // append last bit
                self.lines[start_pos.row + rep_lines_len-1].append(&mut last_bit);

                return vec![line];
            }
            else{ // replaceing multiple lines with multiple lines
                // drain and replace last line
                let rep_line_last = rep_lines.drain((rep_lines.len()-1)..(rep_lines.len())).next().unwrap();
                let last = self.lines[end_pos.row].splice(..end_pos.col, rep_line_last).collect();

                // swap out middle lines and drain them
                let rep_line_mid = rep_lines.drain(1..(rep_lines.len()));
                let mut middle:Vec<Vec<char>> = self.lines.splice( (start_pos.row+1)..end_pos.row, rep_line_mid).collect();

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

    pub fn split_string_to_lines(string:&str)->Vec<Vec<char>>{
        return string.split("\n").map(|s| s.chars().collect()).collect()
    }

    pub fn replace_lines_with_string(&mut self, start:usize, len:usize, string:&str)->TextOp{
        let rep_lines = Self::split_string_to_lines(string);
        let rep_lines_chars = calc_char_count(&rep_lines);
        let lines = self.replace_range(start, len, rep_lines);
        TextOp{
            start:start,
            len:rep_lines_chars,
            lines:lines
        }
    }

    pub fn replace_line_with_string(&mut self, start:usize, row:usize, col:usize, len:usize, string:&str)->TextOp{
        let rep_line:Vec<char> = string.chars().collect();
        let rep_line_chars = rep_line.len();
        let line = self.replace_line(row, col, len, rep_line);
        TextOp{
            start:start,
            len:rep_line_chars,
            lines:vec![line]
        }
    }   

    pub fn replace_with_textop(&mut self, text_op:TextOp)->TextOp{
        let rep_lines_chars = calc_char_count(&text_op.lines);
        let lines = self.replace_range(text_op.start, text_op.len, text_op.lines);
        TextOp{
            start:text_op.start,
            len:rep_lines_chars,
            lines:lines
        }
    }

    pub fn save_buffer(&mut self){
        //let out = self.lines.join("\n");
    }

    pub fn load_buffer(&mut self, data:&Vec<u8>){
        // alright we have to load it and split it on newlines
        if let Ok(utf8_data) = std::str::from_utf8(&data){
            self.lines = Self::split_string_to_lines(&utf8_data.to_string());
            // lets be lazy and redraw all
        }
    }

    pub fn undoredo(&mut self, mut text_undo:TextUndo, cursor_set:&mut TextCursorSet)->TextUndo{
        let mut ops = Vec::new();
        while text_undo.ops.len() > 0{
            let op = text_undo.ops.pop().unwrap();//text_undo.ops.len() - 1);
            ops.push(self.replace_with_textop(op));
        }
        let text_undo_inverse = TextUndo{
            ops:ops,
            grouping:text_undo.grouping,
            cursors:cursor_set.clone()
        };
        cursor_set.set = text_undo.cursors.set.clone();
        cursor_set.last_cursor = text_undo.cursors.last_cursor;
        text_undo_inverse
    }

    // todo make more reuse in these functions
    pub fn undo(&mut self, grouped:bool, cursor_set:&mut TextCursorSet){
        
        if self.undo_stack.len() == 0{
            return;
        }
        let mut last_grouping = TextUndoGrouping::Other;
        let mut first = true;
        while self.undo_stack.len() > 0{
            if !first && !grouped{
                break
            }
            if self.undo_stack.last().unwrap().grouping != last_grouping && !first{
                break
            }
            first = false;
            let text_undo = self.undo_stack.pop().unwrap();
            let wants_grouping = text_undo.grouping.wants_grouping();
            last_grouping = text_undo.grouping.clone();
            let text_redo = self.undoredo(text_undo, cursor_set);
            self.redo_stack.push(text_redo);
            if !wants_grouping{
                break;
            }
        }
    }

    pub fn redo(&mut self, grouped:bool, cursor_set:&mut TextCursorSet){
        if self.redo_stack.len() == 0{
            return;
        }
        let mut last_grouping = TextUndoGrouping::Other;
        let mut first = true;
        while self.redo_stack.len() > 0{
            if !first{
                if self.redo_stack.last().unwrap().grouping != last_grouping || !grouped{
                    break
                }
            }
            first = false;
            let text_redo = self.redo_stack.pop().unwrap();
            let wants_grouping = text_redo.grouping.wants_grouping();
            last_grouping = text_redo.grouping.clone();
            let text_undo = self.undoredo(text_redo, cursor_set);
            self.undo_stack.push(text_undo);
            if !wants_grouping{
                break;
            }
        }
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
            text_buffer:text_buffer,
            line_counter:0,
            offset:0,
            prev:'\0',
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
            self.next_line();
        }
    }

    pub fn next_line(&mut self){
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
