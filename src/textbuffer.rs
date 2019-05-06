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

#[derive(Clone, Copy)]
pub struct TextPos{
    pub row:usize,
    pub col:usize
}

impl TextPos{
    fn dist(&self, other:&TextPos)->f64{
        let dr = (self.row as f64) - (other.row as f64);
        let dc = (self.col as f64) - (other.col as f64);
        (dr*dr+dc*dc).sqrt()
    }
}

#[derive(Clone,PartialEq)]
pub enum TextUndoGrouping{
    Space,
    Newline,
    Character,
    Backspace,
    Delete,
    Block,
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
            TextUndoGrouping::Newline=>true,
            TextUndoGrouping::Character=>true,
            TextUndoGrouping::Backspace=>true,
            TextUndoGrouping::Delete=>true,
            TextUndoGrouping::Block=>false,
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

#[derive(PartialEq,Debug)]
pub enum CharType{
    Whitespace,
    Word,
    Other
}

impl CharType{
    fn from(ch:char)->CharType{
        if ch>='0' && ch <='9' || ch >= 'a' && ch <='z' || ch >= 'A' && ch <='Z' || ch == '_'{
            return CharType::Word
        }
        if ch == ' '|| ch == '\t' || ch == '\n'{
            return CharType::Whitespace
        }
        return CharType::Other
    }

    fn is_prio(&self, rhs:&CharType)->bool{
        match self{
            CharType::Word=>match rhs{
                CharType::Word=>false,
                CharType::Whitespace=>true,
                CharType::Other=>true,
            },
            CharType::Whitespace=>match rhs{
                CharType::Word=>false,
                CharType::Whitespace=>false,
                CharType::Other=>true,
            },
            CharType::Other=>match rhs{
                CharType::Word=>false,
                CharType::Whitespace=>true,
                CharType::Other=>false,
            }
        }
    }
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

    pub fn offset_len_to_text_pos(&self, char_offset:usize, len:usize)->(TextPos, TextPos){
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
                return (
                    TextPos{row:start_row, col:start_col}, 
                    TextPos{row:row, col: (char_offset + len) - char_count}
                );
            }
            char_count = next_char_count;
        }
        (TextPos{row:0, col:0},TextPos{row:0, col:0})
    }


    pub fn text_pos_to_offset(&self, pos:TextPos)->usize{
        let mut char_count = 0;
        if pos.row >= self.lines.len(){
            return self._char_count
        }
        for (ln_row, line) in self.lines.iter().enumerate(){
            if ln_row == pos.row{
                return char_count + line.len().min(pos.col);
            }
            char_count += line.len() + 1;
        }
        0
    }

    pub fn get_nearest_word_range2(&self, offset:usize)->(usize, usize){
        let pos = self.offset_to_text_pos(offset);
        let line = &self.lines[pos.row];
        // lets get the char to the left, if any
        let left_type = CharType::from(if pos.col>0{line[pos.col - 1]} else {'\0'});
        let right_type = CharType::from(if pos.col<line.len(){line[pos.col]} else {'\n'});
        let offset_base = offset - pos.col;
        
        let (mi,ma) = if left_type == right_type{ // scan both ways
            let mut mi = pos.col;
            let mut ma = pos.col;
            while mi>0{
                if left_type != CharType::from(line[mi-1]){
                    break;
                }
                mi -= 1;
            }
            while ma<line.len(){
                if right_type != CharType::from(line[ma]){
                    break;
                }
                ma += 1;
            }
            (mi,ma)
        }
        else if left_type.is_prio(&right_type){ // scan towards the left
            let ma = pos.col;
            let mut mi = pos.col;
            while mi>0{
                if left_type != CharType::from(line[mi-1]){
                    break;
                }
                mi -= 1;
            }
            (mi,ma)
        }
        else{ // scan towards the right
            let mut ma = pos.col;
            let mi = pos.col;
            while ma < line.len(){
                if right_type != CharType::from(line[ma]){
                    break;
                }
                ma += 1;
            }
            (mi,ma)
        };
        (mi+offset_base, ma+offset_base)
    }

    pub fn get_nearest_line_range(&self, offset:usize)->(usize, usize){
        let pos = self.offset_to_text_pos(offset);
        let line = &self.lines[pos.row];
        return (offset - pos.col, line.len() + if pos.row < line.len()-1{1}else{0})
    }

    pub fn get_char_count(&self)->usize{
        self._char_count
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

    fn replace_range(&mut self, start:usize, len:usize, mut rep_lines:Vec<Vec<char>>)->Vec<Vec<char>>{

        let (start_pos, end_pos) = self.offset_len_to_text_pos(start, len);

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

    pub fn calc_max(&mut self, text_buffer:&TextBuffer){
        let pos = text_buffer.offset_to_text_pos(self.head);
        self.max = pos.col;
    }

    pub fn move_home(&mut self, text_buffer:&TextBuffer){
        let pos = text_buffer.offset_to_text_pos(self.head);

        // alright lets walk the line from the left till its no longer 9 or 32
        for (index,ch) in text_buffer.lines[pos.row].iter().enumerate(){
            if *ch != '\t' && *ch != ' '{
                self.head = text_buffer.text_pos_to_offset(TextPos{row:pos.row, col:index});
                self.calc_max(text_buffer);
                return
            }
        }
    }

    pub fn move_end(&mut self, text_buffer:&TextBuffer){
        let pos = text_buffer.offset_to_text_pos(self.head);
        // alright lets walk the line from the left till its no longer 9 or 32
        self.head = text_buffer.text_pos_to_offset(TextPos{row:pos.row, col:text_buffer.lines[pos.row].len()});
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
        let pos = text_buffer.offset_to_text_pos(self.head);
        if pos.row >= line_count {
            self.head = text_buffer.text_pos_to_offset(TextPos{row:pos.row - line_count, col:self.max});
        }
        else{
            self.head = 0;
        }
    }
    
    pub fn move_down(&mut self, line_count:usize, text_buffer:&TextBuffer){
        let pos = text_buffer.offset_to_text_pos(self.head);
        
        if pos.row + line_count < text_buffer.get_line_count() - 1{
            
            self.head = text_buffer.text_pos_to_offset(TextPos{row:pos.row + line_count, col:self.max});
        }
        else{
            self.head = text_buffer.get_char_count() - 1;
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
            if offset + len >= start && offset <end{
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
        cursor.calc_max(text_buffer);
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

    pub fn set_last_cursor_head(&mut self, offset:usize, text_buffer:&TextBuffer){
        let cursor_tail = self.set[self.last_cursor].tail;
        self.set.remove(self.last_cursor);
        self.set_last_cursor(offset, cursor_tail, text_buffer);
    }

    pub fn clear_and_set_last_cursor_head(&mut self, offset:usize, text_buffer:&TextBuffer){
        let cursor_tail = self.set[self.last_cursor].tail;
        self.set.truncate(0);
        self.set_last_cursor(offset, cursor_tail, text_buffer);
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

    pub fn grid_select(&mut self, start_pos:TextPos, end_pos:TextPos, text_buffer:&TextBuffer){
       
        let (left,right) = if start_pos.col < end_pos.col{(start_pos.col, end_pos.col)}
        else{(end_pos.col,start_pos.col)};

        let (top,bottom) = if start_pos.row < end_pos.row{(start_pos.row, end_pos.row)}
        else{(end_pos.row,start_pos.row)};

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
    }

    pub fn set_last_clamp_range(&mut self, range:(usize,usize)){
        self.last_clamp_range = Some(range);
    }

    pub fn clear_last_clamp_range(&mut self){
        self.last_clamp_range = None;
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
        let cursors_clone = self.clone();
        for cursor in &mut self.set{
            let (start, end) = cursor.delta(delta);
            let op = text_buffer.replace_with_string(start, end-start, text);
            delta += cursor.collapse(start, end, op.len);
            ops.push(op);
        }
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:grouping,
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
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Delete,
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
        text_buffer.redo_stack.truncate(0);
        text_buffer.undo_stack.push(TextUndo{
            ops:ops,
            grouping:TextUndoGrouping::Backspace,
            cursors:cursors_clone
        })
    }

    pub fn select_all(&mut self, text_buffer:&mut TextBuffer){
        self.set.truncate(0);
        let mut cursor = Cursor{
            head:0,
            tail:text_buffer.get_char_count(),
            max:0
        };
        cursor.calc_max(text_buffer);
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
