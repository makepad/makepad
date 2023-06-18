use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import crate::theme::*;
    
    DrawLabel= {{DrawLabel}} {
        instance hover: 0.0
        instance focus: 0.0
        text_style: <FONT_LABEL> {}
        fn get_color(self) -> vec4 {
            return
            mix(
                mix(
                    mix(
                        #xFFFFFF33,
                        #xFFFFFF88,
                        self.hover
                    ),
                    #xFFFFFFCC,
                    self.focus
                ),
                #3,
                self.is_empty
            )
        }
    }
    
    TextInput= {{TextInput}} {
        
        draw_cursor: {
            instance focus: 0.0
            uniform border_radius: 0.5
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.fill(mix(#ccc0, #f, self.focus));
                return sdf.result
            }
        }
        
        
        draw_select: {
            instance hover: 0.0
            instance focus: 0.0
            uniform border_radius: 2.0
            fn pixel(self) -> vec4 {
                //return mix(#f00,#0f0,self.pos.y)
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.fill(mix(#5550, #xFFFFFF40, self.focus)); // Pad color
                return sdf.result
            }
        }
        
        cursor_margin_bottom: 3.0,
        cursor_margin_top: 4.0,
        select_pad_edges: 3.0
        cursor_size: 2.0,
        numeric_only: false,
        empty_message: "0",
        draw_bg: {
            instance radius: 2.0
            instance border_width: 0.0
            instance border_color: #3
            instance inset: vec4(0.0,0.0,0.0,0.0)
            
            fn get_color(self)->vec4{
                return self.color
            }
            
            fn get_border_color(self)->vec4{
                return self.border_color
            }
            
            fn pixel(self)->vec4{
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box(
                    self.inset.x + self.border_width,
                    self.inset.y + self.border_width,
                    self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                    self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                    max(1.0, self.radius)
                )
                sdf.fill_keep(self.get_color())
                if self.border_width > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_width)
                }
                return sdf.result;
            }
        },
        layout: {
            clip_x: false,
            clip_y: false,
            padding: {left:10,top:11, right:10, bottom:10}
            align: {y: 0.}
        },
        walk: {
            margin: {top: 5, right: 5}
            width: Fit,
            height: Fit,
            //margin: 0// {left: 0.0, right: 5.0, top: 0.0, bottom: 2.0},
        }
        label_walk: {
            width: Fit,
            height: Fit,
            //margin: 0//{left: 5.0, right: 5.0, top: 0.0, bottom: 2.0},
        }
        align: {
            y: 0.0
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_select: {hover: 0.0}
                        draw_label: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_select: {hover: 1.0}
                        draw_label: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_cursor: {focus: 0.0},
                        draw_select: {focus: 0.0}
                        draw_label: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_cursor: {focus: 1.0},
                        draw_select: {focus: 1.0}
                        draw_label: {focus: 1.0}
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
    #[state] state: LiveState,

    #[live] draw_bg: DrawColor,
    #[live] draw_select: DrawQuad,
    #[live] draw_cursor: DrawQuad,
    #[live] draw_label: DrawLabel,

    #[live] walk: Walk,
    #[live] align: Align,
    #[live] layout: Layout,

    #[live] cursor_size: f64,
    #[live] cursor_margin_bottom: f64,
    #[live] cursor_margin_top: f64,
    #[live] select_pad_edges: f64,
    #[live] empty_message: String,
    #[live] numeric_only: bool,

    #[live] pub read_only: bool,

    #[live] label_walk: Walk,

    #[live] pub text: String,

    #[rust] double_tap_start: Option<(usize, usize)>,
    #[rust] undo_id: u64,
    #[rust] last_undo: Option<UndoItem>,
    #[rust] undo_stack: Vec<UndoItem>,
    #[rust] redo_stack: Vec<UndoItem>,
    #[rust] cursor_tail: usize,
    #[rust] cursor_head: usize
}

impl LiveHook for TextInput{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,TextInput)
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
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
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
    
    pub fn filter_numeric(&self, input:String)->String{
        if self.numeric_only{
            let mut output = String::new();
            for c in input.chars(){
                if c.is_ascii_digit() ||c == '.'{
                    output.push(c);
                }
                else if c == ','{  
                    // some day someone is going to search for this for days
                    output.push('.');
                }
                
            }
            output
        }
        else{
            input
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<TextInputAction> {
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, &mut | _, a | actions.push(a));
        actions
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, TextInputAction)) {
        self.state_handle_event(cx, event);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animate_state(cx, id!(focus.off));
                cx.hide_text_ime();
                dispatch_action(cx, TextInputAction::Return(self.text.clone()));
                dispatch_action(cx, TextInputAction::KeyFocusLost);
            }
            Hit::KeyFocus(_) => {
                self.undo_id += 1;
                self.animate_state(cx, id!(focus.on));
                // select all
                self.select_all();
                self.draw_bg.redraw(cx);
                dispatch_action(cx, TextInputAction::KeyFocus);
            }
            Hit::TextInput(te) => {
                let input = self.filter_numeric(te.input);
                if input.len() == 0{
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
                KeyCode::ArrowLeft => {
                    self.undo_id += 1;
                    if self.cursor_head>0 {
                        self.cursor_head -= 1;
                    }
                    if !ke.modifiers.shift {
                        self.cursor_tail = self.cursor_head;
                    }
                    self.draw_bg.redraw(cx);
                },
                KeyCode::ArrowRight => {
                    self.undo_id += 1;
                    if self.cursor_head < self.text.chars().count() {
                        self.cursor_head += 1;
                    }
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
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            },
            Hit::FingerDown(fe) => {
                cx.set_cursor(MouseCursor::Text);
                self.set_key_focus(cx);
                // ok so we need to calculate where we put the cursor down.
                //elf.
                if let Some(pos) = self.draw_label.closest_offset(cx, fe.abs) {
                    //log!("{} {}", pos, fe.abs);
                    let pos = pos.min(self.text.chars().count());
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
                if let Some(pos) = self.draw_label.closest_offset(cx, fe.abs) {
                    let pos = pos.min(self.text.chars().count());
                    if !fe.mod_shift() && fe.tap_count == 1 && fe.was_tap() {
                        self.cursor_head = pos;
                        self.cursor_tail = self.cursor_head;
                        self.draw_bg.redraw(cx);
                    }
                }
                if fe.was_long_press() {
                    cx.show_clipboard_actions(self.selected_text());
                }
                if fe.is_over && fe.device.has_hovers() {
                    self.animate_state(cx, id!(hover.on));
                }
                else {
                    self.animate_state(cx, id!(hover.off));
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(pos) = self.draw_label.closest_offset(cx, fe.abs) {
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
                        if let Some(pos_start) = self.draw_label.closest_offset(cx, fe.abs_start) {
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
            self.draw_label.is_empty = 1.0;
            self.draw_label.draw_walk(cx, self.label_walk, self.align, &self.empty_message);
        }
        else {
            self.draw_label.is_empty = 0.0;
            self.draw_label.draw_walk(cx, self.label_walk, self.align, &self.text);
        }
        
        let mut turtle = cx.turtle().padded_rect_used();
        turtle.pos.y -= self.cursor_margin_top;
        turtle.size.y += self.cursor_margin_top + self.cursor_margin_bottom;
        // move the IME
        
        let head_x = self.draw_label.get_cursor_pos(cx, 0.0, self.cursor_head)
            .unwrap_or(dvec2(turtle.pos.x, 0.0)).x;
        
        if !self.read_only && self.cursor_head == self.cursor_tail {
            self.draw_cursor.draw_abs(cx, Rect {
                pos: dvec2(head_x - 0.5 * self.cursor_size, turtle.pos.y),
                size: dvec2(self.cursor_size, turtle.size.y)
            });
        }
        
        // draw selection rect
        if self.cursor_head != self.cursor_tail {
            let tail_x = self.draw_label.get_cursor_pos(cx, 0.0, self.cursor_tail)
                .unwrap_or(dvec2(turtle.pos.x, 0.0)).x;
            
            let (left_x, right_x, left, right) = if self.cursor_head < self.cursor_tail {
                (head_x, tail_x, self.cursor_head, self.cursor_tail)
            }
            else {
                (tail_x, head_x, self.cursor_tail, self.cursor_head)
            };
            let char_count = self.draw_label.get_char_count(cx);
            let pad = if left == 0 && right == char_count {self.select_pad_edges}else {0.0};
            
            self.draw_select.draw_abs(cx, Rect {
                pos: dvec2(left_x - 0.5 * self.cursor_size - pad, turtle.pos.y),
                size: dvec2(right_x - left_x + self.cursor_size + 2.0 * pad, turtle.size.y)
            });
        }
        self.draw_bg.end(cx);
        
        if cx.has_key_focus(self.draw_bg.area()) {
            // ok so. if we have the IME we should inject a tracking point
            let ime_x = self.draw_label.get_cursor_pos(cx, 0.5, self.cursor_head)
                .unwrap_or(dvec2(turtle.pos.x, 0.0)).x;
            
            if self.numeric_only{
                cx.hide_text_ime();
            }
            else{
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
    pub fn changed(&self, actions:&WidgetActions) -> String {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let TextInputAction::Change(val) = item.action() {
                return val;
            }
        }
        "".to_string()
    }
    
    pub fn set_text(&self, text:&str){
        if let Some(mut inner) = self.borrow_mut(){
            inner.text.clear();
            inner.text.push_str(text);
        }
    }
    
    pub fn get_text(&self) -> String {
        if let Some(inner) = self.borrow(){
            inner.text.clone()
        } else {
            "".to_string()
        }
    }
}
