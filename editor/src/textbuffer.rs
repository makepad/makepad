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
    ops:Vec<TextOp>,
    grouping:TextUndoGrouping,
    cursors:CursorSet
}

#[derive(Clone)]
pub struct TextOp{
    start:usize,
    len:usize,
    lines:Vec<Vec<char>>,
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

    fn get_range_as_string(&self, start:usize, len:usize, ret:&mut String){
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

    fn replace_line(&mut self, row:usize, start_col:usize, len:usize, rep_line:Vec<char>)->Vec<char>{
        self.lines[row].splice(start_col..(start_col+len), rep_line).collect()
    }

    fn copy_line(&self, row:usize, start_col:usize, len:usize)->Vec<char>{
        let line = &self.lines[row];
        if start_col + len > line.len(){
            self.lines[row][start_col..line.len()].iter().cloned().collect()
        }
        else{
            self.lines[row][start_col..(start_col+len)].iter().cloned().collect()
        }
    }

    fn replace_range(&mut self, start:usize, len:usize, mut rep_lines:Vec<Vec<char>>)->Vec<Vec<char>>{

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

    fn split_string_to_lines(string:&str)->Vec<Vec<char>>{
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

    pub fn undoredo(&mut self, mut text_undo:TextUndo, cursor_set:&mut CursorSet)->TextUndo{
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
    pub fn undo(&mut self, grouped:bool, cursor_set:&mut CursorSet){
        
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

    pub fn redo(&mut self, grouped:bool, cursor_set:&mut CursorSet){
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

#[derive(Clone, Debug, PartialEq)]
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
    
    pub fn clamp_range(&mut self, range:&Option<(usize,usize)>){
        if let Some((off, len)) = range{
            if self.head >= self.tail{
                if self.head < off+len{self.head = off+len}
                if self.tail > *off{self.tail = *off}
            }
            else{
                if self.tail < off+len{self.tail = off+len}
                if self.head > *off{self.head = *off}
            }
        }
    }


    pub fn delta(&self, delta:isize)->(usize,usize){
        let (start,end) = self.order();
        (((start as isize) + delta) as usize, ((end as isize) + delta) as usize)
    }

    pub fn collapse(&mut self, start:usize, end:usize, new_len:usize)->isize{
        self.head = start + new_len;
        self.tail = self.head;
        ( (new_len as isize) - (end - start) as isize )
    }


    pub fn calc_max(&mut self, text_buffer:&TextBuffer, old:(TextPos, usize))->(TextPos, usize){
        let pos = text_buffer.offset_to_text_pos_next(self.head, old.0, old.1);
        self.max = pos.col;
        return (pos, self.head)
    }

    pub fn move_home(&mut self, text_buffer:&TextBuffer){
        let pos = text_buffer.offset_to_text_pos(self.head);

        // alright lets walk the line from the left till its no longer 9 or 32
        for (index,ch) in text_buffer.lines[pos.row].iter().enumerate(){
            if *ch != '\t' && *ch != ' '{
                self.head = text_buffer.text_pos_to_offset(TextPos{row:pos.row, col:index});
                //self.calc_max(text_buffer);
                return
            }
        }
    }

    pub fn move_end(&mut self, text_buffer:&TextBuffer){
        let pos = text_buffer.offset_to_text_pos(self.head);
        // alright lets walk the line from the left till its no longer 9 or 32
        self.head = text_buffer.text_pos_to_offset(TextPos{row:pos.row, col:text_buffer.lines[pos.row].len()});
        //self.calc_max(text_buffer);
    }

    pub fn move_left(&mut self, char_count:usize, _text_buffer:&TextBuffer){
        if self.head >= char_count{
            self.head -= char_count;
        }
        else{
            self.head = 0;
        }
        //self.calc_max(text_buffer);
    }

    pub fn move_right(&mut self, char_count:usize, total_char_count:usize, _text_buffer:&TextBuffer){
        if self.head + char_count < total_char_count{
            self.head += char_count;
        }
        else{
            self.head = total_char_count;
        }
        //self.calc_max(text_buffer);
    }

    pub fn move_up(&mut self, line_count:usize, text_buffer:&TextBuffer){
        let pos = text_buffer.offset_to_text_pos(self.head);
        if pos.row >= line_count {
            self.head = text_buffer.text_pos_to_offset(TextPos{row:pos.row - line_count, col:self.max});
        }
        else{
            self.head = 0;
        }
    }
    
    pub fn move_down(&mut self, line_count:usize,  total_char_count:usize, text_buffer:&TextBuffer){
        let pos = text_buffer.offset_to_text_pos(self.head);
        
        if pos.row + line_count < text_buffer.get_line_count() - 1{
            
            self.head = text_buffer.text_pos_to_offset(TextPos{row:pos.row + line_count, col:self.max});
        }
        else{
            self.head = total_char_count;
        }
    }
}

#[derive(Clone)]
pub struct CursorSet{
    pub set:Vec<Cursor>,
    pub last_cursor:usize,
    pub last_clamp_range:Option<(usize, usize)>
}

impl CursorSet{
    pub fn new()->CursorSet{
        CursorSet{
            set:vec![Cursor{head:0,tail:0,max:0}],
            last_cursor:0,
            last_clamp_range:None
        }
    }

    pub fn get_all_as_string(&self, text_buffer:&TextBuffer)->String{
        let mut ret = String::new();
        for cursor in &self.set{
            let (start, end) = cursor.order();
            text_buffer.get_range_as_string(start, end-start, &mut ret);
        }
        ret
    }

    fn fuse_adjacent(&mut self, text_buffer:&TextBuffer){
        let mut index = 0;
        let mut old_calc = (TextPos{row:0,col:0},0);
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
                    old_calc = self.set[index].calc_max(text_buffer, old_calc);
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

    fn remove_collisions(&mut self, offset:usize, len:usize)->usize{
        // remove any cursor that intersects us
        let mut index = 0;
        loop{
            if index >= self.set.len(){
                return index
            }
            let (start,end) = self.set[index].order();
            if start > offset + len{
                return index
            }
            if offset + len >= start && offset <= end{
                self.set.remove(index); // remove it
            }
            else{
                index += 1;
            }
        }
    }

    // puts the head down
    fn set_last_cursor(&mut self, head:usize, tail:usize, text_buffer:&TextBuffer){
        // widen the head to include the min range
        let (off, len) = if let Some((off, len)) = &self.last_clamp_range{
            (*off, *len)
        }
        else{
            (head, 0)
        };

        // cut out all the collisions with the head range including 'last_cursor'
        let index = self.remove_collisions(off, len);

        // make a new cursor
        let mut cursor = Cursor{
            head:head,
            tail:tail,
            max:0
        };

        // clamp its head/tail to min range
        cursor.clamp_range(&self.last_clamp_range);
        // recompute maximum h pos
        cursor.calc_max(text_buffer, (TextPos{row:0,col:0},0));
        // insert it back into the set
        self.set.insert(index, cursor);
        self.last_cursor = index;
    }

    pub fn add_last_cursor_head_and_tail(&mut self, offset:usize, text_buffer:&TextBuffer){
        self.set_last_cursor(offset, offset, text_buffer);
    }

    pub fn clear_and_set_last_cursor_head_and_tail(&mut self,offset:usize, text_buffer:&TextBuffer){
        self.set.truncate(0);
        self.set_last_cursor(offset, offset, text_buffer);
    }

    pub fn set_last_cursor_head(&mut self, offset:usize, text_buffer:&TextBuffer)->bool{
        if self.set[self.last_cursor].head != offset{
            let cursor_tail = self.set[self.last_cursor].tail;
            self.set.remove(self.last_cursor);
            self.set_last_cursor(offset, cursor_tail, text_buffer);
            true
        }
        else{
            false
        }
    }

    pub fn clear_and_set_last_cursor_head(&mut self, offset:usize, text_buffer:&TextBuffer){
        let cursor_tail = self.set[self.last_cursor].tail;
        self.set.truncate(0);
        self.set_last_cursor(offset, cursor_tail, text_buffer);
    }

    pub fn get_last_cursor_text_pos(&self, text_buffer:&TextBuffer)->TextPos{
        text_buffer.offset_to_text_pos(self.set[self.last_cursor].head)
    }

    pub fn get_last_cursor_order(&self)->(usize,usize){
        self.set[self.last_cursor].order()
    }

    pub fn get_last_cursor_singular(&self)->Option<usize>{
        let cursor = &self.set[self.last_cursor];
        if cursor.head != cursor.tail{
            None
        }
        else{
            Some(cursor.head)
        }
    }

    pub fn is_last_cursor_singular(&self)->bool{
        let cursor = &self.set[self.last_cursor];
        cursor.head == cursor.tail
    }

    pub fn grid_select_corner(&mut self, new_pos:TextPos, text_buffer:&TextBuffer)->TextPos{
        // we need to compute the furthest row/col in our cursor set
        let mut max_dist = 0.0;
        let mut max_pos = TextPos{row:0,col:0};
        for cursor in &self.set{
            let head_pos = text_buffer.offset_to_text_pos(cursor.head);
            let tail_pos = text_buffer.offset_to_text_pos(cursor.tail);
            let head_dist = head_pos.dist(&new_pos);
            let tail_dist = tail_pos.dist(&new_pos);
            if head_dist > tail_dist{
                if head_dist >= max_dist{
                    max_dist = head_dist;
                    max_pos = head_pos;
                }
            }
            else{
                if tail_dist >= max_dist{
                    max_dist = tail_dist;
                    max_pos = tail_pos;
                }
            }
        }
        return max_pos;
    }

    pub fn grid_select(&mut self, start_pos:TextPos, end_pos:TextPos, text_buffer:&TextBuffer)->bool{
       
        let (left,right) = if start_pos.col < end_pos.col{(start_pos.col, end_pos.col)}
        else{(end_pos.col,start_pos.col)};

        let (top,bottom) = if start_pos.row < end_pos.row{(start_pos.row, end_pos.row)}
        else{(end_pos.row,start_pos.row)};

        let change_check = self.set.clone();
        self.set.truncate(0);

        // lets start the cursor gen
        let mut offset = text_buffer.text_pos_to_offset(TextPos{row:top, col:0});
        for row in top..(bottom+1){
            let line = &text_buffer.lines[row];
            if left < line.len(){
                if start_pos.col < end_pos.col{
                    self.set.push(Cursor{
                        tail:offset + left,
                        head:offset + line.len().min(right),
                        max:line.len().min(right)
                    });
                }
                else{
                    self.set.push(Cursor{
                        head:offset + left,
                        tail:offset + line.len().min(right),
                        max:line.len().min(right)
                    });                    
                }
            }
            offset += line.len() + 1;
        }
        // depending on the direction the last cursor remains 
        self.last_cursor = 0;
        self.set != change_check
    }

    pub fn set_last_clamp_range(&mut self, range:(usize,usize)){
        self.last_clamp_range = Some(range);
    }

    pub fn clear_last_clamp_range(&mut self){
        self.last_clamp_range = None;
    }

    pub fn insert_newline_with_indent(&mut self, text_buffer:&mut TextBuffer){
        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let cursors_clone = self.clone();
        let mut old_max = (TextPos{row:0,col:0},0);
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            // lets find where we are as a cursor in the textbuffer
            if start == end{
                // insert spaces till indent level
                let pre_spaces = text_buffer.calc_next_line_indent_depth(start, 4);
                let mut text = String::new();
                text.push_str("\n");
                for _ in 0..pre_spaces{
                    text.push_str(" ");
                }
                let mut next = String::new();
                text_buffer.get_range_as_string(start, 1, &mut next);
                // we have to insert more newlines and spaces because we were between () {} or []
                if next == "}" || next == ")" || next == "]"{
                    let post_spaces = pre_spaces.max(4) - 4;
                    text.push_str("\n");
                    for _ in 0..post_spaces{
                        text.push_str(" ");
                    };
                    let op = text_buffer.replace_lines_with_string(start, end-start, &text);
                    cursor.head += pre_spaces + 1;
                    cursor.tail = cursor.head;
                    delta += (pre_spaces + post_spaces + 2) as isize;
                    ops.push(op);
                }
                else{
                    let op = text_buffer.replace_lines_with_string(start, end-start, &text);
                    delta += cursor.collapse(start, end, op.len);
                    ops.push(op);
                }
            }
            else{
                let op = text_buffer.replace_lines_with_string(start, end-start, "\n");
                delta += cursor.collapse(start, end, op.len);
                ops.push(op);
            };
            
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Newline,
            cursors:cursors_clone
        })
    }

    pub fn replace_text(&mut self, text:&str, text_buffer:&mut TextBuffer){
        let grouping = if text.len() == 1{
            // check if we are space
            let ch = text.chars().next().unwrap();
            if ch == ' '{
                TextUndoGrouping::Space
            }
            else if ch == '\n'{
                TextUndoGrouping::Newline
            }
            else{
                 TextUndoGrouping::Character
            }
        }
        else if text.len() == 0{
            TextUndoGrouping::Cut
        }
        else {
            TextUndoGrouping::Block
        };

        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let mut old_max = (TextPos{row:0,col:0},0);
        let cursors_clone = self.clone();
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            let op = text_buffer.replace_lines_with_string(start, end-start, text);
            delta += cursor.collapse(start, end, op.len);
            ops.push(op);
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:grouping,
            cursors:cursors_clone
        })
    }

    pub fn insert_around(&mut self, pre:&str, post:&str, text_buffer:&mut TextBuffer){
        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let mut old_max = (TextPos{row:0,col:0},0);
        let cursors_clone = self.clone();
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);

            // lets serialize our selection
            let mut text = String::new();
            text.push_str(pre);
            text_buffer.get_range_as_string(start, end-start, &mut text);
            text.push_str(post);
            let op = text_buffer.replace_lines_with_string(start, end-start, &text);

            // we wanna keep the original selection pushed by l
            let pre_chars = pre.chars().count();
            let post_chars = post.chars().count();
            cursor.head += pre_chars;
            cursor.tail += pre_chars;
            delta += (pre_chars+post_chars) as isize;
            ops.push(op);
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Block,
            cursors:cursors_clone
        })
    }

    pub fn overwrite_if_exists(&mut self, thing:&str, text_buffer:&mut TextBuffer){
        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let mut old_max = (TextPos{row:0,col:0},0);
        let cursors_clone = self.clone();
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            // if start == end do overwrite if exists
            let mut next = String::new();
            let thing_chars = thing.chars().count();
            text_buffer.get_range_as_string(start, thing_chars, &mut next);

            let op = if start == end && thing == next{
                // replace thing with next as an op even though its a noop
                text_buffer.replace_lines_with_string(start, thing_chars, thing)
            }
            else{ // normal insert/replace
                text_buffer.replace_lines_with_string(start, end-start, thing)
            };
            delta += cursor.collapse(start, end, op.len);
            ops.push(op);
            old_max = cursor.calc_max(text_buffer, old_max);
       }
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Block,
            cursors:cursors_clone
        })
    }

    pub fn delete(&mut self, text_buffer:&mut TextBuffer){
        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let cursors_clone = self.clone();
        let mut old_max = (TextPos{row:0,col:0},0);
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            if start == end{
                let indent = text_buffer.calc_delete_line_indent_depth(start);
                let op = text_buffer.replace_lines_with_string(start, 1 + indent, "");
                ops.push(op);
                delta += cursor.collapse(start, start+1+indent, 0);
            }
            else if start != end{
                let op = text_buffer.replace_lines_with_string(start, end - start, "");
                ops.push(op);
                delta += cursor.collapse(start, end, 0);
            }
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        let del_pos = self.set[self.last_cursor].head;
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Delete(del_pos),
            cursors:cursors_clone
        })
    }

    pub fn backspace(&mut self, text_buffer:&mut TextBuffer){
        let mut delta:isize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let cursors_clone = self.clone();
        let mut old_max = (TextPos{row:0,col:0},0);
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            if start == end && start > 0{
                // check our indent depth
                let (new_start, new_len) = text_buffer.calc_backspace_line_indent_depth_and_pair(start);
                let op = text_buffer.replace_lines_with_string(new_start, new_len, "");
                ops.push(op);
                delta += cursor.collapse(new_start,new_start + new_len, 0);
            }
            else if start != end{
                let op = text_buffer.replace_lines_with_string(start, end - start, "");
                ops.push(op);
                delta += cursor.collapse(start, end, 0);
            }
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Backspace,
            cursors:cursors_clone
        })
    }

    pub fn insert_tab(&mut self, text_buffer:&mut TextBuffer, tab_str:&str){
        let mut delta:usize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let tab_str_chars = tab_str.chars().count();
        let cursors_clone = self.clone();
        let mut old_max = (TextPos{row:0,col:0},0);
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta as isize);
            /*
            i find these things really bad UX. so lets not.
            if start == end{ // just insert 4 spaces
                // check our indent depth
                let op = text_buffer.replace_lines_with_string(start, end-start, tab_str);
                delta += cursor.collapse(start, end, op.len);
                ops.push(op);
            }
            else if start != end{ // either indent the lines, OR replace
                let start_pos = text_buffer.offset_to_text_pos_next(start, old_max.0, old_max.1);
                let end_pos = text_buffer.offset_to_text_pos_next(end, start_pos, start);
                if start_pos.row == end_pos.row{ // its a single line replace with 4 chars
                    let op = text_buffer.replace_lines_with_string(start, end - start, tab_str);
                    ops.push(op);
                    delta += cursor.collapse(start, end, tab_str_chars);
                }
                else{ // tab indent the lines
            */
            let start_pos = text_buffer.offset_to_text_pos_next(start, old_max.0, old_max.1);
            let end_pos = text_buffer.offset_to_text_pos_next(end, start_pos, start);
            let mut off = start - start_pos.col;
            for row in start_pos.row..(end_pos.row+1){
                // ok so how do we compute the actual op offset of this line
                let op = text_buffer.replace_line_with_string(off, row, 0, 0, tab_str);
                off += text_buffer.lines[row].len() + 1;
                ops.push(op);
            }
            // figure out which way the cursor is
            if cursor.head > cursor.tail{
                cursor.tail += tab_str_chars + delta;
                cursor.head += (end_pos.row - start_pos.row + 1) * tab_str_chars + delta;
            }
            else{
                cursor.tail += (end_pos.row - start_pos.row + 1) * tab_str_chars + delta;
                cursor.head += tab_str_chars + delta;
            }
            delta += ((end_pos.row - start_pos.row) + 1) * tab_str_chars;
            //    }
            //}
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Tab,
            cursors:cursors_clone
        })
    }

    pub fn remove_tab(&mut self, text_buffer:&mut TextBuffer, num_spaces:usize){

        let mut delta:usize = 0; // rolling delta to displace cursors 
        let mut ops = Vec::new();
        let cursors_clone = self.clone();
        let mut old_max = (TextPos{row:0,col:0},0);
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(-(delta as isize));
            let start_pos = text_buffer.offset_to_text_pos_next(start, old_max.0, old_max.1);
            let end_pos = text_buffer.offset_to_text_pos_next(end, start_pos, start);
            let mut off = start - start_pos.col;
            let mut total_cut_len = 0;

            for row in start_pos.row..(end_pos.row+1){
                let indents = text_buffer.calc_line_indent_depth(row);
                let cut_len = num_spaces.min(indents);
                if cut_len > 0{
                    total_cut_len += cut_len;
                    let op = text_buffer.replace_line_with_string(off, row, 0, num_spaces, "");
                    if cursor.head > off{
                        cursor.head -= cut_len;
                    }
                    if cursor.tail > off{
                        cursor.tail -= cut_len;
                    }
                    ops.push(op);
                }
                off += text_buffer.lines[row].len() + 1;
            }
            cursor.head -= delta;
            cursor.tail -= delta;
            delta += total_cut_len;
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Tab,
            cursors:cursors_clone
        })
        
    }


    pub fn select_all(&mut self, text_buffer:&mut TextBuffer){
        self.set.truncate(0);
        let mut cursor = Cursor{
            head:0,
            tail:text_buffer.calc_char_count(),
            max:0
        };
        self.last_cursor = 0;
        cursor.calc_max(text_buffer, (TextPos{row:0,col:0},0));
        self.set.push(cursor);
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
        let total_char_count = text_buffer.calc_char_count();
        for cursor in &mut self.set{
            cursor.move_down(line_count, total_char_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_left(&mut self, char_count:usize, only_head:bool, text_buffer:&TextBuffer){
        let mut old_max = (TextPos{row:0,col:0},0);
        for cursor in &mut self.set{
            cursor.move_left(char_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_right(&mut self,char_count:usize, only_head:bool, text_buffer:&TextBuffer){
        let mut old_max = (TextPos{row:0,col:0},0);
        let total_char_count = text_buffer.calc_char_count();
        for cursor in &mut self.set{
            cursor.move_right(char_count, total_char_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
            old_max = cursor.calc_max(text_buffer, old_max);
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn get_nearest_token_chunk_boundary(left:bool, offset:usize, chunks:&Vec<TokenChunk>)->usize{
        for i in 0..chunks.len(){
            // if we are in the chunk, decide what to do
            if offset >= chunks[i].offset && offset < chunks[i].offset + chunks[i].len{
                if left{ // we want to to the beginning of the prev token
                    if offset > chunks[i].offset{
                        return chunks[i].offset
                    }
                    if i ==0  {
                        return 0
                    }
                    if offset == chunks[i].offset{
                        if chunks[i-1].token_type == TokenType::Whitespace && i>1{
                            return chunks[i-2].offset// + chunks[i-2].len
                        }
                        return chunks[i-1].offset
                    }
                    return chunks[i-1].offset + chunks[i-1].len
                }
                else{ // jump right

                    if i < chunks.len() - 1 && chunks[i].token_type == TokenType::Whitespace{
                        return chunks[i+1].offset + chunks[i+1].len;
                    }
                    return chunks[i].offset + chunks[i].len
                }
            }
        };
        0
    }

    pub fn get_nearest_token_chunk(offset:usize, token_chunks:&Vec<TokenChunk>)->Option<TokenChunk>{
        for i in 0..token_chunks.len(){
            if token_chunks[i].token_type == TokenType::Whitespace{
                if offset == token_chunks[i].offset && i > 0{ // at the start of whitespace
                    return Some(token_chunks[i-1].clone())
                }
                else if offset == token_chunks[i].offset + token_chunks[i].len && i < token_chunks.len()-1{
                    return Some(token_chunks[i+1].clone())
                }
            };

            if offset >= token_chunks[i].offset && offset < token_chunks[i].offset + token_chunks[i].len{
                let i = if token_chunks[i].token_type == TokenType::Newline && i > 0{i - 1}else{i};
                let pair_token = token_chunks[i].pair_token;
                if pair_token > i{
                    return Some(TokenChunk{
                        token_type:TokenType::Block,
                        offset: token_chunks[i].offset,
                        len:token_chunks[pair_token].len + (token_chunks[pair_token].offset - token_chunks[i].offset),
                        pair_token: 0
                    })
                }
                return Some(token_chunks[i].clone());
            }
        };
        None
    }

    pub fn move_left_nearest_token(&mut self, only_head:bool, token_chunks:&Vec<TokenChunk>, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            // take the cursor head and find nearest token left
            let pos = CursorSet::get_nearest_token_chunk_boundary(true, cursor.head, token_chunks);
            cursor.head = pos;
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_right_nearest_token(&mut self, only_head:bool, token_chunks:&Vec<TokenChunk>, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            // take the cursor head and find nearest token left
            let pos = CursorSet::get_nearest_token_chunk_boundary(false, cursor.head, token_chunks);
            cursor.head = pos;
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn get_token_highlight(&self, text_buffer:&TextBuffer, token_chunks:&Vec<TokenChunk>)->Vec<char>{
        let cursor = &self.set[self.last_cursor];
       
        if let Some(chunk) = CursorSet::get_nearest_token_chunk(cursor.head, token_chunks){
            let add = match chunk.token_type{
                TokenType::Whitespace=>false,
                TokenType::Newline=>false,
                TokenType::Keyword=>false,
                TokenType::Flow=>false,
                TokenType::Identifier=>true,
                TokenType::Call=>true,
                TokenType::TypeName=>true,

                TokenType::String=>true,
                TokenType::Number=>true,

                TokenType::DocComment=>false,
                TokenType::Comment=>false,
                
                TokenType::ParenOpen=>false,
                TokenType::ParenClose=>false,
                TokenType::Operator=>false,
                TokenType::Delimiter=>false,
                TokenType::Block=>false,
                TokenType::Unexpected=>false,
            };
            if !add{
                vec![]
            }
            else{
                let start_pos = text_buffer.offset_to_text_pos(chunk.offset);
                
                text_buffer.copy_line(start_pos.row, start_pos.col, chunk.len)
            }
        }
        else{
            vec![]
        }
    }

    pub fn get_selection_highlight(&self, text_buffer:&TextBuffer)->Vec<char>{
        let cursor = &self.set[self.last_cursor];
        if cursor.head != cursor.tail{
            let (start,end) = cursor.order();
            let start_pos = text_buffer.offset_to_text_pos(start);
            let end_pos = text_buffer.offset_to_text_pos_next(end, start_pos, start);
            if start_pos.row != end_pos.row{
                return vec![]
            };
            let buf = text_buffer.copy_line(start_pos.row, start_pos.col, end_pos.col - start_pos.col);
            let mut only_spaces = true;
            for ch in &buf{
                if *ch != ' '{
                    only_spaces = false;
                    break;
                }
            };
            if only_spaces{
                return vec![]
            }
            return buf
        }
        else{
            return vec![]
        }
    }


}

#[derive(Clone, PartialEq)]
pub enum TokenType{
    Whitespace,
    Newline,
    Keyword,
    Flow,
    Identifier,
    Call,
    TypeName,

    String,
    Number,

    DocComment,
    Comment,

    ParenOpen,
    ParenClose,
    Operator,
    Delimiter,
    Block,

    Unexpected
}

#[derive(Clone)]
pub struct TokenChunk{
    pub token_type:TokenType,
    pub offset:usize,
    pub pair_token:usize,
    pub len:usize,
}
