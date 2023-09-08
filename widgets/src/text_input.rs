use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    DrawLabel = {{DrawLabel}} {}
    TextInputBase = {{TextInput}} {}
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
    External(u64),
    Cut(u64),
}


#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawLabel {
    #[deref] draw_super: DrawText,
    #[live] is_empty: f32,
}


#[derive(Live)]
pub struct TextInput {
    #[animator] animator: Animator,
    
    #[live] draw_bg: DrawColor,
    #[live] draw_select: DrawQuad,
    #[live] draw_cursor: DrawQuad,
    #[live] draw_text: DrawLabel,
    
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    
    #[live] label_align: Align,
    
    #[live] cursor_size: f64,
    #[live] cursor_margin_bottom: f64,
    #[live] cursor_margin_top: f64,
    #[live] select_pad_edges: f64,
    #[live] empty_message: String,
    #[live] numeric_only: bool,
    #[live] on_focus_select_all: bool,
    #[live] pub read_only: bool,
    
    //#[live] label_walk: Walk,
    
    #[live] pub text: String,
    #[live] ascii_only: bool,
    #[rust] double_tap_start: Option<(usize, usize)>,
    #[rust] undo_id: u64,
    
    #[rust] last_undo: Option<UndoItem>,
    #[rust] undo_stack: Vec<UndoItem>,
    #[rust] redo_stack: Vec<UndoItem>,
    #[rust] cursor_tail: usize,
    #[rust] cursor_head: usize
}

impl LiveHook for TextInput {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, TextInput)
    }
}

impl Widget for TextInput {
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}
    /*fn bind_read(&mut self, _cx: &mut Cx, nodes: &[LiveNode]) {
        
        if let Some(LiveValue::Float(v)) = nodes.read_path(&self.bind) {
            self.set_internal(*v as f32);
        }
    }*/
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
    
    
    fn text(&self) -> String {
        self.text.clone()
    }
    
    fn set_text(&mut self, v: &str) {
        self.filter_input(&v, None);
    }
}

#[derive(Clone, PartialEq, WidgetAction)]
pub enum TextInputAction {
    Change(String),
    Return(String),
    Escape,
    KeyFocus,
    KeyFocusLost,
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
    
    pub fn select_all(&mut self) {
        self.cursor_tail = 0;
        self.cursor_head = self.text.chars().count();
    }
    
    fn create_undo_item(&mut self, undo_group: UndoGroup) -> UndoItem {
        UndoItem {
            undo_group: undo_group,
            text: self.text.clone(),
            cursor_head: self.cursor_head,
            cursor_tail: self.cursor_tail
        }
    }
    
    pub fn create_external_undo(&mut self) {
        self.create_undo(UndoGroup::External(self.undo_id))
    }
    
    pub fn create_undo(&mut self, undo_group: UndoGroup) {
        if self.read_only {
            return
        }
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
    
    pub fn select_word(&mut self, around: usize) {
        let mut first_ws = Some(0);
        let mut last_ws = None;
        let mut after_center = false;
        for (i, c) in self.text.chars().enumerate() {
            last_ws = Some(i + 1);
            if i >= around {
                after_center = true;
            }
            if c.is_whitespace() {
                last_ws = Some(i);
                if after_center {
                    break;
                }
                first_ws = Some(i + 1);
            }
        }
        if let Some(first_ws) = first_ws {
            if let Some(last_ws) = last_ws {
                self.cursor_tail = first_ws;
                self.cursor_head = last_ws;
            }
        }
    }
    
    pub fn change(&mut self, cx: &mut Cx, s: &str, dispatch_action: &mut dyn FnMut(&mut Cx, TextInputAction)) {
        if self.read_only {
            return
        }
        self.replace_text(s);
        dispatch_action(cx, TextInputAction::Change(self.text.clone()));
        self.draw_bg.redraw(cx);
    }
    
    pub fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.draw_bg.area());
    }
    
    pub fn filter_input(&mut self, input: &str, output: Option<&mut String>) {
        let output = if let Some(output) = output {
            output
        }
        else {
            &mut self.text
        };
        output.clear();
        if self.ascii_only {
            for c in input.as_bytes() {
                if *c>31 && *c<127 {
                    output.push(*c as char);
                }
            }
        }
        else if self.numeric_only {
            let mut output = String::new();
            for c in input.chars() {
                if c.is_ascii_digit() || c == '.' {
                    output.push(c);
                }
                else if c == ',' {
                    // some day someone is going to search for this for days
                    output.push('.');
                }
            }
        }
        else {
            output.push_str(input);
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<TextInputAction> {
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, &mut | _, a | actions.push(a));
        actions
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, TextInputAction)) {
        self.animator_handle_event(cx, event);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                cx.hide_text_ime();
                dispatch_action(cx, TextInputAction::Return(self.text.clone()));
                dispatch_action(cx, TextInputAction::KeyFocusLost);
            }
            Hit::KeyFocus(_) => {
                self.undo_id += 1;
                self.animator_play(cx, id!(focus.on));
                // select all
                if self.on_focus_select_all {
                    self.select_all();
                }
                self.draw_bg.redraw(cx);
                dispatch_action(cx, TextInputAction::KeyFocus);
            }
            Hit::TextInput(te) => {
                let mut input = String::new();
                self.filter_input(&te.input, Some(&mut input));
                if input.len() == 0 {
                    return
                }
                let last_undo = self.last_undo.take();
                if te.replace_last {
                    self.undo_id += 1;
                    self.create_undo(UndoGroup::TextInput(self.undo_id));
                    if let Some(item) = last_undo {
                        self.consume_undo_item(item);
                    }
                }
                else {
                    if input == " " {
                        self.undo_id += 1;
                    }
                    // if this one follows a space, it still needs to eat it
                    self.create_undo(UndoGroup::TextInput(self.undo_id));
                }
                self.change(cx, &input, dispatch_action);
            }
            Hit::TextCopy(ce) => {
                self.undo_id += 1;
                *ce.response.borrow_mut() = Some(self.selected_text());
            }
            Hit::TextCut(tc) => {
                self.undo_id += 1;
                if self.cursor_head != self.cursor_tail {
                    *tc.response.borrow_mut() = Some(self.selected_text());
                    self.create_undo(UndoGroup::Cut(self.undo_id));
                    self.change(cx, "", dispatch_action);
                }
            }
            Hit::KeyDown(ke) => match ke.key_code {
                
                KeyCode::Tab => {
                    // dispatch_action(cx, self, TextInputAction::Tab(key.mod_shift));
                }
                KeyCode::ReturnKey => {
                    cx.hide_text_ime();
                    dispatch_action(cx, TextInputAction::Return(self.text.clone()));
                },
                KeyCode::Escape => {
                    dispatch_action(cx, TextInputAction::Escape);
                },
                KeyCode::KeyZ if ke.modifiers.logo || ke.modifiers.shift => {
                    if self.read_only {
                        return
                    }
                    self.undo_id += 1;
                    if ke.modifiers.shift {
                        self.redo();
                    }
                    else {
                        self.undo();
                    }
                    dispatch_action(cx, TextInputAction::Change(self.text.clone()));
                    self.draw_bg.redraw(cx);
                }
                KeyCode::KeyA if ke.modifiers.logo || ke.modifiers.control => {
                    self.undo_id += 1;
                    self.cursor_tail = 0;
                    self.cursor_head = self.text.chars().count();
                    self.draw_bg.redraw(cx);
                }
                KeyCode::ArrowLeft => if !ke.modifiers.logo {
                    
                    self.undo_id += 1;
                    if self.cursor_head>0 {
                        self.cursor_head -= 1;
                    }
                    if !ke.modifiers.shift {
                        self.cursor_tail = self.cursor_head;
                    }
                    self.draw_bg.redraw(cx);
                },
                KeyCode::ArrowRight => if !ke.modifiers.logo {
                    self.undo_id += 1;
                    if self.cursor_head < self.text.chars().count() {
                        self.cursor_head += 1;
                    }
                    if !ke.modifiers.shift {
                        self.cursor_tail = self.cursor_head;
                    }
                    self.draw_bg.redraw(cx);
                }
                KeyCode::ArrowDown => if !ke.modifiers.logo {
                    self.undo_id += 1;
                    // we need to figure out what is below our current cursor
                    if let Some(pos) = self.draw_text.get_cursor_pos(cx, 0.0, self.cursor_head) {
                        if let Some(pos) = self.draw_text.closest_offset(cx, dvec2(pos.x, pos.y + self.draw_text.get_line_spacing() * 1.5)) {
                            self.cursor_head = pos;
                            if !ke.modifiers.shift {
                                self.cursor_tail = self.cursor_head;
                            }
                            self.draw_bg.redraw(cx);
                        }
                    }
                },
                KeyCode::ArrowUp => if !ke.modifiers.logo {
                    self.undo_id += 1;
                    // we need to figure out what is below our current cursor
                    if let Some(pos) = self.draw_text.get_cursor_pos(cx, 0.0, self.cursor_head) {
                        if let Some(pos) = self.draw_text.closest_offset(cx, dvec2(pos.x, pos.y - self.draw_text.get_line_spacing() * 0.5)) {
                            self.cursor_head = pos;
                            if !ke.modifiers.shift {
                                self.cursor_tail = self.cursor_head;
                            }
                            self.draw_bg.redraw(cx);
                        }
                    }
                },
                KeyCode::Home => if !ke.modifiers.logo {
                    self.undo_id += 1;
                    self.cursor_head = 0;
                    if !ke.modifiers.shift {
                        self.cursor_tail = self.cursor_head;
                    }
                    self.draw_bg.redraw(cx);
                }
                KeyCode::End => if !ke.modifiers.logo {
                    self.undo_id += 1;
                    self.cursor_head = self.text.chars().count();
                    
                    if !ke.modifiers.shift {
                        self.cursor_tail = self.cursor_head;
                    }
                    self.draw_bg.redraw(cx);
                }
                KeyCode::Backspace => {
                    self.create_undo(UndoGroup::Backspace(self.undo_id));
                    if self.cursor_head == self.cursor_tail {
                        if self.cursor_tail > 0 {
                            self.cursor_tail -= 1;
                        }
                    }
                    self.change(cx, "", dispatch_action);
                }
                KeyCode::Delete => {
                    self.create_undo(UndoGroup::Delete(self.undo_id));
                    if self.cursor_head == self.cursor_tail {
                        if self.cursor_head < self.text.chars().count() {
                            self.cursor_head += 1;
                        }
                    }
                    self.change(cx, "", dispatch_action);
                }
                _ => ()
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Text);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(fe) => {
                cx.set_cursor(MouseCursor::Text);
                self.set_key_focus(cx);
                // ok so we need to calculate where we put the cursor down.
                //elf.
                if let Some(pos) = self.draw_text.closest_offset(cx, fe.abs) {
                    //log!("{} {}", pos, fe.abs);
                    let pos = pos.min(self.text.chars().count());
                    if fe.tap_count == 1 {
                        if pos != self.cursor_head {
                            self.cursor_head = pos;
                            if !fe.modifiers.shift {
                                self.cursor_tail = pos;
                            }
                        }
                        self.draw_bg.redraw(cx);
                    }
                    if fe.tap_count == 2 {
                        // lets select the word.
                        self.select_word(pos);
                        self.double_tap_start = Some((self.cursor_head, self.cursor_tail));
                    }
                    if fe.tap_count == 3 {
                        self.select_all();
                    }
                    self.draw_bg.redraw(cx);
                }
            },
            Hit::FingerUp(fe) => {
                self.double_tap_start = None;
                if let Some(pos) = self.draw_text.closest_offset(cx, fe.abs) {
                    let pos = pos.min(self.text.chars().count());
                    if !fe.modifiers.shift && fe.tap_count == 1 && fe.was_tap() {
                        self.cursor_head = pos;
                        self.cursor_tail = self.cursor_head;
                        self.draw_bg.redraw(cx);
                    }
                }
                if fe.was_long_press() {
                    cx.show_clipboard_actions(self.selected_text());
                }
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(pos) = self.draw_text.closest_offset(cx, fe.abs) {
                    let pos = pos.min(self.text.chars().count());
                    if fe.tap_count == 2 {
                        let (head, tail) = self.double_tap_start.unwrap();
                        // ok so. now we do a word select and merge the selection
                        self.select_word(pos);
                        if head > self.cursor_head {
                            self.cursor_head = head
                        }
                        if tail < self.cursor_tail {
                            self.cursor_tail = tail;
                        }
                        self.draw_bg.redraw(cx);
                    }
                    else if fe.tap_count == 1 {
                        if let Some(pos_start) = self.draw_text.closest_offset(cx, fe.abs_start) {
                            let pos_start = pos_start.min(self.text.chars().count());
                            
                            self.cursor_head = pos_start;
                            self.cursor_tail = self.cursor_head;
                        }
                        if pos != self.cursor_head {
                            self.cursor_head = pos;
                        }
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            _ => ()
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        
        self.draw_bg.begin(cx, walk, self.layout);
        let turtle_rect = cx.turtle().rect();
        
        // this makes sure selection goes behind the text
        self.draw_select.append_to_draw_call(cx);
        
        if self.text.len() == 0 {
            self.draw_text.is_empty = 1.0;
            self.draw_text.draw_walk(cx, Walk::size(self.walk.width, self.walk.height), self.label_align, &self.empty_message);
        }
        else {
            self.draw_text.is_empty = 0.0;
            self.draw_text.draw_walk(cx, Walk::size(
                self.walk.width,
                self.walk.height
            ), self.label_align, &self.text);
        }
        
        let mut turtle = cx.turtle().padded_rect_used();
        turtle.pos.y -= self.cursor_margin_top;
        turtle.size.y += self.cursor_margin_top + self.cursor_margin_bottom;
        // move the IME
        let line_spacing = self.draw_text.get_line_spacing();
        let top_drop = self.draw_text.get_font_size() * 0.2;
        let head = self.draw_text.get_cursor_pos(cx, 0.0, self.cursor_head)
            .unwrap_or(dvec2(turtle.pos.x, 0.0));
        
        if !self.read_only && self.cursor_head == self.cursor_tail {
            self.draw_cursor.draw_abs(cx, Rect {
                pos: dvec2(head.x - 0.5 * self.cursor_size, head.y - top_drop),
                size: dvec2(self.cursor_size, line_spacing)
            });
        }
        
        // draw selection rects
        
        if self.cursor_head != self.cursor_tail {
            let top_drop = self.draw_text.get_font_size() * 0.3;
            let bottom_drop = self.draw_text.get_font_size() * 0.1;
            
            let (start, end) = self.sorted_cursor();
            let rects = self.draw_text.get_selection_rects(cx, start, end, dvec2(0.0, -top_drop), dvec2(0.0, bottom_drop));
            for rect in rects {
                self.draw_select.draw_abs(cx, rect);
            }
        }
        self.draw_bg.end(cx);
        
        if  cx.has_key_focus(self.draw_bg.area()) {
            // ok so. if we have the IME we should inject a tracking point
            let ime_x = self.draw_text.get_cursor_pos(cx, 0.5, self.cursor_head)
                .unwrap_or(dvec2(turtle.pos.x, 0.0)).x;
            
            if self.numeric_only {
                cx.hide_text_ime();
            }
            else {
                let ime_abs = dvec2(ime_x, turtle.pos.y);
                cx.show_text_ime(self.draw_bg.area(), ime_abs - turtle_rect.pos);
            }
        }
        
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default())
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct TextInputRef(WidgetRef);

impl TextInputRef {
    pub fn changed(&self, actions: &WidgetActions) -> Option<String> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let TextInputAction::Change(val) = item.action() {
                return Some(val);
            }
        }
        None
    }
    
}
