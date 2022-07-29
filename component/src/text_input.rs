#![allow(unused)]
use {
    crate::{
        makepad_derive_frame::*,
        makepad_platform::*,
        button_logic::*,
        frame_traits::*,
    }
};

live_register!{
    import makepad_platform::shader::std::*;
    import crate::theme::*;
    
    TextInput: {{TextInput}} {
        
        cursor: {
            instance focus: 0.0
            const BORDER_RADIUS: 0.5
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    BORDER_RADIUS
                )
                sdf.fill(mix(#ccc0, #cccf, self.focus));
                return sdf.result
            }
        }
        
        label: {
            instance hover: 0.0
            instance focus: 0.0
            instance selected: 1.0
            
            text_style: FONT_LABEL {}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #b,
                        self.hover
                    ),
                    mix(
                        #9,
                        #f,
                        self.selected
                    ),
                    self.focus
                )
            }
        }
        
        select: {
            instance hover: 0.0
            instance focus: 0.0
            
            const BORDER_RADIUS: 2.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    BORDER_RADIUS
                )
                sdf.fill(mix(#aaa3, #fff7, self.focus));
                return sdf.result
            }
        }
        cursor_size: 2.0,
        bg: {
            shape: Box
            color: #5
            radius: 2
        },
        walk: {
            width: Fit,
            height: Fill,
            margin: {left: 1.0, right: 5.0, top: 0.0, bottom: 2.0},
        }
        label_walk: {
            width: Fit,
            height: Fill,
            margin: {left: 5.0, right: 5.0, top: 2.0, bottom: 2.0},
        }
        align: {
            y: 0.5
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        select: {hover: 0.0}
                        label: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        select: {hover: 1.0}
                        label: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        cursor: {focus: 0.0},
                        select: {focus: 0.0}
                        label: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        cursor: {focus: 1.0},
                        select: {focus: 1.0}
                        label: {focus: 1.0}
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct UndoItem {
    text: String,
    undo_group: UndoGroup,
    cursor_head: usize,
    cursor_tail: usize
}

#[derive(PartialEq, Copy, Clone)]
pub enum UndoGroup {
    TextInput(u64),
    Backspace(u64),
    Delete(u64),
    Cut(u64),
}

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(TextInput))]
pub struct TextInput {
    state: State,
    
    bg: DrawShape,
    select: DrawQuad,
    cursor: DrawQuad,
    label: DrawText,
    
    walk: Walk,
    align: Align,
    layout: Layout,
    
    cursor_size: f32,
    
    label_walk: Walk,
    
    pub text: String,
    #[rust] undo_id: u64,
    #[rust] last_undo: Option<UndoItem>,
    #[rust] undo_stack: Vec<UndoItem>,
    #[rust] redo_stack: Vec<UndoItem>,
    #[rust] cursor_tail: usize,
    #[rust] cursor_head: usize
}
impl LiveHook for TextInput {
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        //nodes.debug_print(index,100);
        None
    }
}

#[derive(Copy, Clone, PartialEq, FrameAction)]
pub enum TextInputAction {
    None
}

impl TextInput {
    
    pub fn sorted_cursor(&self) -> (usize, usize) {
        if self.cursor_head < self.cursor_tail {
            (self.cursor_head, self.cursor_tail)
        }
        else {
            (self.cursor_tail, self.cursor_head)
        }
    }
    
    pub fn selected_text(&mut self) -> String {
        let mut ret = String::new();
        let (left, right) = self.sorted_cursor();
        for (i, c) in self.text.chars().enumerate() {
            if i >= left && i< right {
                ret.push(c);
            }
            if i >= right {
                break;
            }
        }
        ret
    }
    
    fn consume_undo_item(&mut self, item: UndoItem) {
        self.text = item.text;
        self.cursor_head = item.cursor_head;
        self.cursor_tail = item.cursor_tail;
    }
    
    pub fn undo(&mut self) {
        if let Some(item) = self.undo_stack.pop() {
            let redo_item = self.create_undo_item(item.undo_group);
            self.consume_undo_item(item.clone());
            self.redo_stack.push(redo_item);
        }
    }
    
    pub fn redo(&mut self) {
        if let Some(item) = self.redo_stack.pop() {
            let undo_item = self.create_undo_item(item.undo_group);
            self.consume_undo_item(item.clone());
            self.undo_stack.push(undo_item);
        }
    }
    
    fn create_undo_item(&mut self, undo_group: UndoGroup) -> UndoItem {
        UndoItem {
            undo_group: undo_group,
            text: self.text.clone(),
            cursor_head: self.cursor_head,
            cursor_tail: self.cursor_tail
        }
    }
    
    pub fn create_undo(&mut self, undo_group: UndoGroup) {
        self.redo_stack.clear();
        let new_item = self.create_undo_item(undo_group);
        if let Some(item) = self.undo_stack.last_mut() {
            if item.undo_group != undo_group {
                self.last_undo = Some(new_item.clone());
                self.undo_stack.push(new_item);
            }
            else {
                self.last_undo = Some(new_item);
            }
        }
        else {
            self.last_undo = Some(new_item.clone());
            self.undo_stack.push(new_item);
        }
    }
    
    pub fn replace_text(&mut self, inp: &str) {
        let mut new = String::new();
        let (left, right) = self.sorted_cursor();
        let mut chars_inserted = 0;
        let mut inserted = false;
        for (i, c) in self.text.chars().enumerate() {
            // cursor insertion point
            if i == left {
                inserted = true;
                for c in inp.chars() {
                    chars_inserted += 1;
                    new.push(c);
                }
            }
            // outside of the selection so copy
            if i < left || i >= right {
                new.push(c);
            }
        }
        if !inserted { // end of string or empty string
            for c in inp.chars() {
                chars_inserted += 1;
                new.push(c);
            }
        }
        self.cursor_head = left + chars_inserted;
        self.cursor_tail = self.cursor_head;
        self.text = new;
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, TextInputAction)) {
        self.state_handle_event(cx, event);
        match event.hits(cx, self.bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animate_state(cx, ids!(focus.off));
            }
            Hit::KeyFocus(_) => {
                self.undo_id += 1;
                self.animate_state(cx, ids!(focus.on));
            }
            Hit::TextInput(te) => {
                let last_undo = self.last_undo.take();
                if te.replace_last {
                    self.undo_id += 1;
                    self.create_undo(UndoGroup::TextInput(self.undo_id));
                    if let Some(item) = last_undo {
                        self.consume_undo_item(item);
                    }
                }
                else {
                    if te.input == " " {
                        self.undo_id += 1;
                    }
                    // if this one follows a space, it still needs to eat it
                    self.create_undo(UndoGroup::TextInput(self.undo_id));
                }
                self.replace_text(&te.input);
                self.bg.redraw(cx);
            }
            Hit::TextCopy(ce) => {
                self.undo_id += 1;
                *ce.response.borrow_mut() = Some(self.selected_text())
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::KeyZ if ke.mod_logo() || ke.mod_control() => {
                    self.undo_id += 1;
                    if ke.mod_shift() {
                        self.redo();
                    }
                    else {
                        self.undo();
                    }
                    self.bg.redraw(cx);
                }
                KeyCode::KeyA if ke.mod_logo() || ke.mod_control() => {
                    self.undo_id += 1;
                    self.cursor_tail = 0;
                    self.cursor_head = self.text.chars().count();
                    self.bg.redraw(cx);
                }
                KeyCode::KeyX if ke.mod_logo() || ke.mod_control() => {
                    self.undo_id += 1;
                    if self.cursor_head != self.cursor_tail {
                        self.create_undo(UndoGroup::Cut(self.undo_id));
                        self.bg.redraw(cx);
                    }
                }
                KeyCode::ArrowLeft => {
                    self.undo_id += 1;
                    if self.cursor_head>0 {
                        self.cursor_head -= 1;
                    }
                    if !ke.mod_shift() {
                        self.cursor_tail = self.cursor_head;
                    }
                    self.bg.redraw(cx);
                },
                KeyCode::ArrowRight => {
                    self.undo_id += 1;
                    if self.cursor_head < self.text.chars().count() {
                        self.cursor_head += 1;
                    }
                    if !ke.mod_shift() {
                        self.cursor_tail = self.cursor_head;
                    }
                    self.bg.redraw(cx);
                }
                KeyCode::Backspace => {
                    self.create_undo(UndoGroup::Backspace(self.undo_id));
                    if self.cursor_head == self.cursor_tail {
                        if self.cursor_tail > 0 {
                            self.cursor_tail -= 1;
                        }
                    }
                    self.replace_text("");
                    self.bg.redraw(cx);
                }
                KeyCode::Delete => {
                    self.create_undo(UndoGroup::Delete(self.undo_id));
                    if self.cursor_head == self.cursor_tail {
                        if self.cursor_head < self.text.chars().count() {
                            self.cursor_head += 1;
                        }
                    }
                    self.replace_text("");
                    self.bg.redraw(cx);
                }
                _ => ()
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Text);
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, ids!(hover.off));
            },
            Hit::FingerDown(fe) => {
                cx.set_cursor(MouseCursor::Text);
                cx.set_key_focus(self.bg.area());
                // ok so we need to calculate where we put the cursor down.
                //elf.
                if let Some(pos) = self.label.closest_offset(cx, fe.abs) {
                    self.cursor_head = pos;
                    if !fe.mod_shift() {
                        self.cursor_tail = self.cursor_head;
                    }
                    self.bg.redraw(cx);
                }
            },
            Hit::FingerUp(fe) => {
                if fe.is_over && fe.finger_type.has_hovers() {
                    self.animate_state(cx, ids!(hover.on));
                }
                else {
                    self.animate_state(cx, ids!(hover.off));
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(pos) = self.label.closest_offset(cx, fe.abs) {
                    if pos != self.cursor_head {
                        self.cursor_head = pos;
                        self.bg.redraw(cx);
                    }
                }
            }
            _ => ()
        }
    }
    
    pub fn cursor_to_screen(&self, cx: &Cx2d, cursor_pos: usize) -> Option<f32> {
        if cursor_pos >= self.text.len() {
            if self.text.len() == 0 {
                None
            }
            else {
                let rect = self.label.character_rect(cx, cursor_pos - 1).unwrap();
                Some(rect.pos.x + rect.size.x)
            }
            
        } else {
            let rect = self.label.character_rect(cx, cursor_pos).unwrap();
            Some(rect.pos.x)
        }
    }
    
    
    pub fn cursor_to_ime_pos(&self, cx: &Cx2d, cursor_pos: usize) -> Option<f32> {
        if cursor_pos >= self.text.len() {
            if self.text.len() == 0 {
                None
            }
            else {
                let rect = self.label.character_rect(cx, cursor_pos - 1).unwrap();
                Some(rect.pos.x + 0.5 * rect.size.x)
            }
            
        } else {
            let rect = self.label.character_rect(cx, cursor_pos.max(1) - 1).unwrap();
            Some(rect.pos.x + 0.5 * rect.size.x)
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.bg.begin(cx, walk, self.layout);
        
        // this makes sure selection goes behind the text
        self.select.append_to_draw_call(cx);
        
        self.label.draw_walk(cx, self.label_walk, self.align, &self.text);
        
        let turtle = cx.turtle().rect();
        
        // move the IME
        if cx.has_key_focus(self.bg.area()) {
            let ime_x = self.cursor_to_ime_pos(cx, self.cursor_head).unwrap_or(turtle.pos.x);
            cx.show_text_ime(vec2(ime_x, turtle.pos.y));
        }
        
        let head_x = self.cursor_to_screen(cx, self.cursor_head).unwrap_or(turtle.pos.x);
        self.cursor.draw_abs(cx, Rect {
            pos: vec2(head_x - 0.5 * self.cursor_size, turtle.pos.y),
            size: vec2(self.cursor_size, turtle.size.y)
        });
        
        // draw selection rect
        if self.cursor_head != self.cursor_tail {
            let tail_x = self.cursor_to_screen(cx, self.cursor_tail).unwrap_or(turtle.pos.x);
            let (left_x, right_x) = if self.cursor_head < self.cursor_tail {
                (head_x, tail_x)
            }
            else {
                (tail_x, head_x)
            };
            self.select.draw_abs(cx, Rect {
                pos: vec2(left_x - 0.5 * self.cursor_size, turtle.pos.y),
                size: vec2(right_x - left_x + self.cursor_size, turtle.size.y)
            });
        }
        self.bg.end(cx);
    }
}
