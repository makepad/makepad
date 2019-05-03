//use widgets::*;

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
    pub _char_count: usize
}

#[derive(Clone)]
pub struct TextUndo{
    ops:Vec<TextOp>,
    cursors:CursorSet
}

#[derive(Clone)]
pub struct TextOp{
    start:usize,
    len:usize,
    lines:Vec<Vec<char>>,
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

    pub fn offset_len_to_row_col(&self, char_offset:usize, len:usize)->(usize,usize,usize,usize){
        let mut char_count = 0;
        let mut start_row = 0;
        let mut start_col = 0;
        let mut begin_found = false;
        for (row,line) in self.lines.iter().enumerate(){
            let next_char_count = char_count + line.len() + 1;
            if !begin_found && next_char_count > char_offset{
                start_row = row;
                start_col = char_offset - char_count;
                begin_found = true;
            }
            if begin_found && next_char_count > char_offset + len{
                return (start_row, start_col, row, (char_offset + len) - char_count);
            }
            char_count = next_char_count;
        }
        (0,0,0,0)
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
/*
    fn get_range_as_string(&self, start:usize, len:usize)->String{
        let mut ret = String::new();
        let (mut row, mut col) = self.offset_to_row_col(start);
        for _ in 0..len{
            let line = &self.lines[row];
            if col >= line.len(){
                ret.push('\n');
                col = 0;
                row += 1;
                if row >= self.lines.len(){
                    return ret;
                }
            }
            else{
                ret.push(line[col]);
                col += 1;
            }
        };
        return ret;
    }*/

    fn replace_range(&mut self, start:usize, len:usize, mut rep_lines:Vec<Vec<char>>)->Vec<Vec<char>>{

        let (start_row, start_col, end_row, end_col) = self.offset_len_to_row_col(start, len);

        if start_row == end_row && rep_lines.len() == 1{ // replace in one line
            let rep_line_zero = rep_lines.drain(0..1).next().unwrap();
            let line = self.lines[start_row].splice(start_col..end_col, rep_line_zero).collect();
            return vec![line];
        }
        else{
            if rep_lines.len() == 1{ // we are replacing multiple lines with one line
                // drain first line
                let rep_line_zero = rep_lines.drain(0..1).next().unwrap();

                // replace it in the first
                let first = self.lines[start_row].splice(start_col.., rep_line_zero).collect();

                // collect the middle ones
                let mut middle:Vec<Vec<char>> = self.lines.drain((start_row+1)..(end_row)).collect();

                // cut out the last bit
                let last:Vec<char> = self.lines[start_row+1].drain(0..end_col).collect();
                
                // last line bit
                let mut last_line = self.lines.drain((start_row+1)..(start_row+2)).next().unwrap();

                // merge start_row+1 into start_row
                self.lines[start_row].append(&mut last_line);

                // concat it all together
                middle.insert(0, first);
                middle.push(last);

                return middle
            }
            else if start_row == end_row{ // replacing single line with multiple lines
                let mut last_bit:Vec<char> =  self.lines[start_row].drain(end_col..).collect();// but we have co drain end_col..

                // replaced first line
                let rep_lines_len = rep_lines.len();
                let rep_line_first:Vec<char> = rep_lines.drain(0..1).next().unwrap();
                let line = self.lines[start_row].splice(start_col.., rep_line_first).collect();

                // splice in middle rest
                let rep_line_mid = rep_lines.drain(0..(rep_lines.len()));
                self.lines.splice( (start_row+1)..(start_row+1), rep_line_mid);
                
                // append last bit
                self.lines[start_row + rep_lines_len-1].append(&mut last_bit);

                return vec![line];
            }
            else{ // replaceing multiple lines with multiple lines
                // drain and replace last line
                let rep_line_last = rep_lines.drain((rep_lines.len()-1)..(rep_lines.len())).next().unwrap();
                let last = self.lines[end_row].splice(..end_col, rep_line_last).collect();

                // swap out middle lines and drain them
                let rep_line_mid = rep_lines.drain(1..(rep_lines.len()));
                let mut middle:Vec<Vec<char>> = self.lines.splice( (start_row+1)..end_row, rep_line_mid).collect();

                // drain and replace first line
                let rep_line_zero = rep_lines.drain(0..1).next().unwrap();
                let first = self.lines[start_row].splice(start_col.., rep_line_zero).collect();

                // concat it all together
                middle.insert(0, first);
                middle.push(last);
                return middle
            }
        }
    }

    fn split_string_to_lines(string:&str)->Vec<Vec<char>>{
        return string.split("\n").map(|s| s.chars().collect()).collect()
    }

    pub fn compute_char_count(lines:&Vec<Vec<char>>)->usize{
        let mut char_count = 0;
        for line in lines{
            char_count += line.len()
        }
        char_count += lines.len() - 1; // invisible newline chars
        char_count
    }

    pub fn replace_with_string(&mut self, start:usize, len:usize, string:&str)->TextOp{
        let rep_lines = Self::split_string_to_lines(string);
        let rep_lines_chars = Self::compute_char_count(&rep_lines);
        let lines = self.replace_range(start, len, rep_lines);
        // ok now we have to replace start, len with data
        self._char_count = Self::compute_char_count(&self.lines);
        TextOp{
            start:start,
            len:rep_lines_chars,
            lines:lines
        }
    }

    pub fn replace_with_textop(&mut self, text_op:TextOp)->TextOp{
        let rep_lines_chars = Self::compute_char_count(&text_op.lines);
        let lines = self.replace_range(text_op.start, text_op.len, text_op.lines);
        self._char_count = Self::compute_char_count(&self.lines);
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
        self._char_count = Self::compute_char_count(&self.lines);
    }

    pub fn undoredo(&mut self, mut text_undo:TextUndo, cursor_set:&mut CursorSet)->TextUndo{
        let mut ops = Vec::new();
        while text_undo.ops.len() > 0{
            let op = text_undo.ops.pop().unwrap();//text_undo.ops.len() - 1);
            ops.push(self.replace_with_textop(op));
        }
        let text_undo_inverse = TextUndo{
            ops:ops,
            cursors:cursor_set.clone()
        };
        cursor_set.set = text_undo.cursors.set.clone();
        cursor_set.last_cursor = text_undo.cursors.last_cursor;
        text_undo_inverse
    }

    pub fn undo(&mut self, cursor_set:&mut CursorSet){
        if self.undo_stack.len() == 0{
            return;
        }
        let text_undo = self.undo_stack.pop().unwrap();
        let text_redo = self.undoredo(text_undo, cursor_set);
        self.redo_stack.push(text_redo);
    }

    pub fn redo(&mut self, cursor_set:&mut CursorSet){
        if self.redo_stack.len() == 0{
            return;
        }
        let text_redo = self.redo_stack.pop().unwrap();
        let text_undo = self.undoredo(text_redo, cursor_set);
        self.undo_stack.push(text_undo);
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

#[derive(Clone)]
pub struct Cursor{
    pub head:usize,
    pub tail:usize,
    pub max:usize
}

impl Cursor{
    pub fn has_selection(&self)->bool{
        self.head != self.tail
    }

    pub fn order(&self)->(usize,usize){
        if self.head > self.tail{
            (self.tail,self.head)
        }
        else{
            (self.head,self.tail)
        }
    }

    pub fn delta(&self, delta:isize)->(usize,usize){
        let (start,end) = self.order();
        (((start as isize) + delta) as usize, ((end as isize) + delta) as usize)
    }

    pub fn collapse(&mut self, start:usize, end:usize, new_len:usize)->isize{
        self.head = start + new_len;
        self.tail = self.head;
        ((end - start) as isize - (new_len as isize))
    }

    pub fn calc_max(&mut self, text_buffer:&TextBuffer){
        let (_row,col) = text_buffer.offset_to_row_col(self.head);
        self.max = col;
    }

    pub fn move_home(&mut self, text_buffer:&TextBuffer){
        let (row, _col) = text_buffer.offset_to_row_col(self.head);

        // alright lets walk the line from the left till its no longer 9 or 32
        for (pos,ch) in text_buffer.lines[row].iter().enumerate(){
            if *ch != '\t' && *ch != ' '{
                self.head = text_buffer.row_col_to_offset(row, pos);
                self.calc_max(text_buffer);
                return
            }
        }
    }

    pub fn move_end(&mut self, text_buffer:&TextBuffer){
        let (row, _col) = text_buffer.offset_to_row_col(self.head);
        // alright lets walk the line from the left till its no longer 9 or 32
        self.head = text_buffer.row_col_to_offset(row, text_buffer.lines[row].len());
        self.calc_max(text_buffer);
    }

    pub fn move_left(&mut self, char_count:usize,  text_buffer:&TextBuffer){
        if self.head >= char_count{
            self.head -= char_count;
        }
        else{
            self.head = 0;
        }
        self.calc_max(text_buffer);
    }

    pub fn move_right(&mut self, char_count:usize, text_buffer:&TextBuffer){
        if self.head + char_count < text_buffer.get_char_count() - 1{
            self.head += char_count;
        }
        else{
            self.head = text_buffer.get_char_count() - 1;
        }
        self.calc_max(text_buffer);
    }

    pub fn move_up(&mut self, line_count:usize, text_buffer:&TextBuffer){
        let (row,_col) = text_buffer.offset_to_row_col(self.head);
        if row >= line_count {
            self.head = text_buffer.row_col_to_offset(row - line_count, self.max);
        }
        else{
            self.head = 0;
        }
    }
    
    pub fn move_down(&mut self, line_count:usize, text_buffer:&TextBuffer){
        let (row,_col) = text_buffer.offset_to_row_col(self.head);
        
        if row + line_count < text_buffer.get_line_count() - 1{
            
            self.head = text_buffer.row_col_to_offset(row + line_count, self.max);
        }
        else{
            self.head = text_buffer.get_char_count() - 1;
        }
    }
}

#[derive(Clone)]
pub struct CursorSet{
    pub set:Vec<Cursor>,
    pub last_cursor:usize
}

impl CursorSet{
    pub fn new()->CursorSet{
        CursorSet{
            set:vec![Cursor{head:0,tail:0,max:0}],
            last_cursor:0
        }
    }

    pub fn fuse_adjacent(&mut self, text_buffer:&TextBuffer){
        let mut index = 0;
        loop{
            if self.set.len() < 2 || index >= self.set.len() - 1{ // no more pairs
                return
            }
            // get the pair data
            let (my_start,my_end) = self.set[index].order();
            let (next_start,next_end) = self.set[index+1].order();
            if my_end >= next_start{ // fuse them together
                // check if we are mergin down or up
                if my_end < next_end{ // otherwise just remove the next
                    if self.set[index].tail>self.set[index].head{ // down
                        self.set[index].head = my_start;
                        self.set[index].tail = next_end;
                    }
                    else{ // up
                    self.set[index].head = next_end;
                    self.set[index].tail = my_start;
                    }
                    self.set[index].calc_max(text_buffer);
                    // remove the next item
                }
                if self.last_cursor > index{
                    self.last_cursor -= 1;
                }
                self.set.remove(index + 1);
            }
            index += 1;
        }
    }

    pub fn remove_collision(&mut self, offset:usize)->usize{
        // remove any cursor that intersects us
        let mut index = 0;
        loop{
            if index >= self.set.len(){
                return index
            }
            let (start,end) = self.set[index].order();
            if offset < start{
                return index
            }
            if offset >= start && offset <=end{
                if self.last_cursor > index{ // we remove a cursor before the last_cursor
                    self.last_cursor = self.last_cursor - 1;
                    self.set.remove(index);
                }
                else if self.last_cursor != index{ // it something after it so it doesnt matter
                    self.set.remove(index);
                }
                else{ // we are the last_cursor
                    index += 1;
                }
            }
            else{
                index += 1;
            }
        }
    }

    // puts the head down
    pub fn begin_cursor_drag(&mut self, add:bool, offset:usize, text_buffer:&TextBuffer){
        if !add{
            self.set.truncate(0);
        }

        let index = self.remove_collision(offset);
        
        self.set.insert(index, Cursor{
            head:offset,
            tail:offset,
            max:0
        });
        self.last_cursor = index;
        self.set[index].calc_max(text_buffer);
    }

    pub fn update_cursor_drag(&mut self, offset:usize, text_buffer:&TextBuffer){

        // remove any cursor that intersects us
        self.remove_collision(offset);

        self.set[self.last_cursor].head = offset;
        self.set[self.last_cursor].calc_max(text_buffer);
    }

    pub fn end_cursor_drag(&mut self, _text_buffer:&TextBuffer){
    }

    pub fn replace_text(&mut self, text:&str, text_buffer:&mut TextBuffer){
        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let cursors_clone = self.clone();
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            let op = text_buffer.replace_with_string(start, end-start, text);
            delta += cursor.collapse(start, end, op.len);
            ops.push(op);
        }
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            cursors:cursors_clone
        })
    }

    pub fn delete(&mut self, text_buffer:&mut TextBuffer){
        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let cursors_clone = self.clone();
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            if start == end{
                let op = text_buffer.replace_with_string(start, 1, "");
                ops.push(op);
                delta += cursor.collapse(start, start+1, 0);
            }
            else if start != end{
                let op = text_buffer.replace_with_string(start, end - start, "");
                ops.push(op);
                delta += cursor.collapse(start, end, 0);
            }
        }
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            cursors:cursors_clone
        })
    }

    pub fn backspace(&mut self, text_buffer:&mut TextBuffer){
        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let cursors_clone = self.clone();
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            if start == end && start > 0{
                let op = text_buffer.replace_with_string(start - 1, 1, "");
                ops.push(op);
                delta += cursor.collapse(start - 1, start, 0);
            }
            else if start != end{
                let op = text_buffer.replace_with_string(start, end - start, "");
                ops.push(op);
                delta += cursor.collapse(start, end, 0);
            }
        }
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            cursors:cursors_clone
        })
    }

    pub fn move_home(&mut self,only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            cursor.move_home(text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_end(&mut self,only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            cursor.move_end(text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_up(&mut self, line_count:usize, only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            cursor.move_up(line_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_down(&mut self,line_count:usize, only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            cursor.move_down(line_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_left(&mut self, char_count:usize, only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            cursor.move_left(char_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_right(&mut self,char_count:usize, only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            cursor.move_right(char_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }
}
