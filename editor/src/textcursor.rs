use widget::*;
use crate::textbuffer::*;

#[derive(Clone, Debug, PartialEq)]
pub struct TextCursor{
    pub head:usize,
    pub tail:usize,
    pub max:usize
}

impl TextCursor{
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
pub struct TextCursorSet{
    pub set:Vec<TextCursor>,
    pub last_cursor:usize,
    pub last_clamp_range:Option<(usize, usize)>
}

impl TextCursorSet{
    pub fn new()->TextCursorSet{
        TextCursorSet{
            set:vec![TextCursor{head:0,tail:0,max:0}],
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
        let mut cursor = TextCursor{
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
        // check if we dont already have exactly that cursor. ifso just remove it
        if self.set.len()>1{
            for i in 0..self.set.len(){
                if self.set[i].head == self.set[i].tail && self.set[i].head == offset{
                    self.set.remove(i);
                    self.last_cursor = self.set.len() - 1;
                    return
                }
            }
        }
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
                    self.set.push(TextCursor{
                        tail:offset + left,
                        head:offset + line.len().min(right),
                        max:line.len().min(right)
                    });
                }
                else{
                    self.set.push(TextCursor{
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
        let mut cursor = TextCursor{
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
            let pos = TextCursorSet::get_nearest_token_chunk_boundary(true, cursor.head, token_chunks);
            cursor.head = pos;
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn move_right_nearest_token(&mut self, only_head:bool, token_chunks:&Vec<TokenChunk>, text_buffer:&TextBuffer){
        for cursor in &mut self.set{
            // take the cursor head and find nearest token left
            let pos = TextCursorSet::get_nearest_token_chunk_boundary(false, cursor.head, token_chunks);
            cursor.head = pos;
            if !only_head{cursor.tail = cursor.head}
        }
        self.fuse_adjacent(text_buffer)
    }

    pub fn get_token_highlight(&self, text_buffer:&TextBuffer, token_chunks:&Vec<TokenChunk>)->Vec<char>{
        let cursor = &self.set[self.last_cursor];
       
        if let Some(chunk) = TextCursorSet::get_nearest_token_chunk(cursor.head, token_chunks){
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


#[derive(Clone)]
pub struct DrawSel{
    pub index:usize,
    pub rc:Rect,
}

#[derive(Clone)]
pub struct DrawCursors{
    pub head:usize,
    pub start:usize,
    pub end:usize,
    pub next_index:usize,
    pub left_top:Vec2,
    pub right_bottom:Vec2,
    pub last_w:f32,
    pub first:bool,
    pub empty:bool,
    pub cursors:Vec<Rect>,
    pub last_cursor:Option<usize>,
    pub selections:Vec<DrawSel>
}

impl DrawCursors{
    pub fn new()->DrawCursors{
        DrawCursors{
            start:0,
            end:0,
            head:0,
            first:true,
            empty:true,
            next_index:0,
            left_top:Vec2::zero(),
            right_bottom:Vec2::zero(),
            last_w:0.0,
            cursors:Vec::new(),
            selections:Vec::new(),
            last_cursor:None
        }
    }

    pub fn term(&mut self, cursors:&Vec<TextCursor>){
        self.next_index = cursors.len();
    }

    pub fn set_next(&mut self, cursors:&Vec<TextCursor>)->bool{
        if self.next_index < cursors.len(){
            self.emit_selection();
            let cursor = &cursors[self.next_index];
            let (start,end) = cursor.order();
            self.start = start;
            self.end = end;
            self.head = cursor.head;
            self.next_index += 1;
            self.last_w = 0.0;
            self.right_bottom.y = 0.;
            self.first = true;
            self.empty = true;
            true
        }
        else{
            false
        }
    }

    pub fn emit_cursor(&mut self, x:f32, y:f32, h:f32){
        self.cursors.push(Rect{
            x:x,
            y:y,
            w:1.5,
            h:h
        })
    }

    pub fn emit_selection(&mut self){
        if !self.first{
            self.first = true;
            if !self.empty{
                self.selections.push(DrawSel{
                    index:self.next_index - 1,
                    rc:Rect{
                        x:self.left_top.x,
                        y:self.left_top.y,
                        w:(self.right_bottom.x - self.left_top.x),
                        h:self.right_bottom.y - self.left_top.y
                    }
                })
            }
            self.right_bottom.y = 0.;
        }
    }

     pub fn emit_selection_new_line(&mut self){
        if !self.first{
            self.first = true;
            self.selections.push(DrawSel{
                index:self.next_index - 1,
                rc:Rect{
                    x:self.left_top.x,
                    y:self.left_top.y,
                    w:(self.right_bottom.x - self.left_top.x) + self.last_w,
                    h:self.right_bottom.y - self.left_top.y
                }
            });
            self.right_bottom.y = 0.;
        }
    }

    pub fn process_cursor(&mut self, last_cursor:usize, offset:usize, x:f32, y:f32, h:f32){
        if offset == self.head{ // emit a cursor
            if self.next_index > 0 && self.next_index - 1 == last_cursor{
                self.last_cursor = Some(self.cursors.len());
            }
            self.emit_cursor(x, y, h);
        }
    }

    pub fn process_geom(&mut self, x:f32, y:f32, w:f32, h:f32){
        if self.first{ // store left top of rect
            self.first = false;
            self.left_top.x = x;
            self.left_top.y = y;
            self.empty = true;
        }
        else{
            self.empty = false;
        }
        // current right/bottom
        self.last_w = w;
        self.right_bottom.x = x;
        if y + h > self.right_bottom.y{
            self.right_bottom.y = y + h;
        }
    }

    pub fn process_newline(&mut self){
        if !self.first{ // we have some selection data to emit
           self.emit_selection_new_line();
           self.first = true;
        }
    }

    pub fn mark_text_select_only(&mut self, cursors:&Vec<TextCursor>, offset:usize, x:f32, y:f32, w:f32, h:f32){
        // check if we need to skip cursors
        while offset >= self.end{ // jump to next cursor
            if offset == self.end{ // process the last bit here
                self.process_geom(x, y, w, h);
                self.emit_selection();
            }
            if !self.set_next(cursors){ // cant go further
                return
            }
        }
        // in current cursor range, update values
        if offset >= self.start && offset <= self.end{
            self.process_geom(x, y, w, h);
            if offset == self.end{
                self.emit_selection();
            }
        }
    }

    pub fn mark_text_with_cursor(&mut self, cursors:&Vec<TextCursor>, ch:char, offset:usize, x:f32, y:f32, w:f32, h:f32, last_cursor:usize)->f32{
        // check if we need to skip cursors
        while offset >= self.end{ // jump to next cursor
            if offset == self.end{ // process the last bit here
                self.process_cursor(last_cursor, offset, x, y, h);
                self.process_geom(x, y, w, h);
                self.emit_selection();
            }
            if !self.set_next(cursors){ // cant go further
                return 0.0
            }
        }
        // in current cursor range, update values
        if offset >= self.start && offset <= self.end{
            self.process_cursor(last_cursor, offset, x, y, h);
            self.process_geom(x, y, w, h);
            if offset == self.end{
                self.emit_selection();
            }
            if ch == '\n'{
                return 0.0
            }
            else if ch == ' ' && offset < self.end{
                return 2.0
            }
        }
        return 0.0
    }
}