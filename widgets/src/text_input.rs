use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    },
    unicode_segmentation::{GraphemeCursor, UnicodeSegmentation},
};

live_design!{
    DrawLabel = {{DrawLabel}} {}
    TextInputBase = {{TextInput}} {}
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawLabel {
    #[deref] draw_super: DrawText,
    #[live] is_empty: f32,
}

#[derive(Live, LiveHook, Widget)]
pub struct TextInput {
    #[animator] animator: Animator,
    
    #[redraw] #[live] draw_bg: DrawColor,
    #[live] draw_label: DrawLabel,
    #[live] draw_selection: DrawQuad,
    #[live] draw_cursor: DrawQuad,
    
    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[live] label_align: Align,

    #[live] cursor_width: f64,

    #[live] pub is_read_only: bool,
    #[live] pub is_numeric_only: bool,
    #[live] pub empty_text: String,
    #[live] pub text: String,

    #[rust] cursor: Cursor,
    #[rust] history: History,
}

impl TextInput {
    pub fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.draw_bg.area());
    }

    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }

    pub fn select_all(&mut self) {
        self.set_cursor(Cursor {
            head: IndexAffinity {
                index: self.text.len(),
                affinity: Affinity::After,
            },
            tail: IndexAffinity {
                index: 0,
                affinity: Affinity::Before,
            },
        });
    }

    pub fn filter_input(&mut self, input: String) -> String {
        if self.is_numeric_only {
            input.chars().filter_map(|char| {
                match char {
                    '.' | ',' => Some('.'),
                    char if char.is_ascii_digit() => Some(char),
                    _ => None,
                }
            }).collect()
        } else {
            input
        }
    }

    pub fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group();
    }

    fn position_to_index_affinity(&self, cx: &mut Cx2d, width: f64, position: DVec2) -> IndexAffinity {
        self.draw_label.position_to_index_affinity(
            cx,
            Walk::size(self.walk.width,self.walk.height),
            self.label_align,
            width,
            &self.text,
            position,
        )
    }

    fn cursor_position(&self, cx: &mut Cx2d, width: f64) -> DVec2 {
        self.draw_label.index_affinity_to_position(
            cx,
            Walk::size(self.walk.width,self.walk.height),
            self.label_align,
            width,
            &self.text,
            self.cursor.head,
        )
    }

    fn move_cursor_left(&mut self, is_select: bool) {
        let Some(index) = prev_grapheme_boundary(&self.text, self.cursor.head.index) else {
            return;
        };
        self.move_cursor_to(
            IndexAffinity {
                index,
                affinity: Affinity::After,
            },
            is_select
        );
    }

    fn move_cursor_right(&mut self, is_select: bool) {
        let Some(index) = next_grapheme_boundary(&self.text, self.cursor.head.index) else {
            return;
        };
        self.move_cursor_to(
            IndexAffinity {
                index,
                affinity: Affinity::Before,
            },
            is_select
        );
    }

    fn move_cursor_up(&mut self, cx: &mut Cx2d, width: f64, is_select: bool) {
        let position = self.cursor_position(cx, width);
        let line_spacing = self.draw_label.line_spacing(cx);
        let index_affinity = self.position_to_index_affinity(cx, width, DVec2 {
            x: position.x,
            y: position.y - 0.5 * line_spacing,
        });
        self.move_cursor_to(index_affinity, is_select)
    }

    fn move_cursor_down(&mut self, cx: &mut Cx2d, width: f64, is_select: bool) {
        let position = self.cursor_position(cx, width);
        let line_spacing = self.draw_label.line_spacing(cx);
        let index_affinity = self.position_to_index_affinity(cx, width, DVec2 {
            x: position.x,
            y: position.y + 1.5 * line_spacing,
        });
        self.move_cursor_to(index_affinity, is_select);
    }

    fn move_cursor_to(&mut self, index_affinity: IndexAffinity, is_select: bool) {
        self.cursor.head = index_affinity;
        if !is_select {
            self.cursor.tail = self.cursor.head;
        }
        self.history.force_new_edit_group();
    }

    fn select_word(&mut self) {
        if self.cursor.head.index < self.cursor.tail.index { 
            self.cursor.head = IndexAffinity {
                index: self.ceil_word_boundary(self.cursor.head.index),
                affinity: Affinity::After,
            };
        } else if self.cursor.head.index > self.cursor.tail.index {
            self.cursor.head = IndexAffinity {
                index: self.floor_word_boundary(self.cursor.head.index),
                affinity: Affinity::Before,
            };
        } else {
            self.cursor.tail = IndexAffinity {
                index: self.ceil_word_boundary(self.cursor.head.index),
                affinity: Affinity::After,
            };
            self.cursor.head = IndexAffinity {
                index: self.floor_word_boundary(self.cursor.head.index),
                affinity: Affinity::Before,
            };
        }
    }

    fn ceil_word_boundary(&self, index: usize) -> usize {
        let mut prev_word_boundary_index = 0;
        for (word_boundary_index, _) in self.text.split_word_bound_indices() {
            if word_boundary_index > index {
                return prev_word_boundary_index;
            }
            prev_word_boundary_index = word_boundary_index;
        }
        prev_word_boundary_index
    }

    fn floor_word_boundary(&self, index: usize) -> usize {
        let mut prev_word_boundary_index = self.text.len();
        for (word_boundary_index, _) in self.text.split_word_bound_indices().rev() {
            if word_boundary_index < index {
                return prev_word_boundary_index;
            }
            prev_word_boundary_index = word_boundary_index;
        }
        prev_word_boundary_index
    }

    fn apply_edit(&mut self, edit: Edit) {
        self.cursor.head.index = edit.start + edit.replace_with.len();
        self.cursor.tail = self.cursor.head;
        self.history.apply_edit(edit, &mut self.text);
    }

    fn undo(&mut self) {
        if let Some(cursor) = self.history.undo(self.cursor, &mut self.text) {
            self.cursor = cursor;
        }
    }

    fn redo(&mut self) {
        if let Some(cursor) = self.history.redo(self.cursor, &mut self.text) {
            self.cursor = cursor;
        }
    }
}

impl Widget for TextInput {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let rect = self.draw_bg.area().rect(cx);
        let padded_rect = Rect {
            pos: rect.pos + self.layout.padding.left_top(),
            size: rect.size - self.layout.padding.left_top(),
        };

        let uid = self.widget_uid();

        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, id!(focus.on));
                self.force_new_edit_group();
                // TODO: Select all if necessary
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocus);
            },
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                cx.hide_text_ime();
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocusLost);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {
                    shift: is_select,
                    ..
                },
                ..
            }) => {
                self.move_cursor_left(is_select);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {
                    shift: is_select,
                    ..
                },
                ..
            }) => {
                self.move_cursor_right( is_select);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {
                    shift: is_select,
                    ..
                },
                ..
            }) => {
                let event = DrawEvent::default();
                let mut cx = Cx2d::new(cx, &event);
                self.move_cursor_up(&mut cx, padded_rect.size.x, is_select);
                self.draw_bg.redraw(&mut cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {
                    shift: is_select,
                    ..
                },
                ..
            }) => {
                let event = DrawEvent::default();
                let mut cx = Cx2d::new(cx, &event);
                self.move_cursor_down(&mut cx, padded_rect.size.x, is_select);
                self.draw_bg.redraw(&mut cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Home,
                ..
            }) => {
                self.move_cursor_to(
                    IndexAffinity {
                        index: 0,
                        affinity: Affinity::Before,
                    },
                    false
                );
                self.history.force_new_edit_group();
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::End,
                ..
            }) => {
                self.move_cursor_to(
                    IndexAffinity {
                        index: self.text.len(),
                        affinity: Affinity::After,
                    },
                    false
                );
                self.history.force_new_edit_group();
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: KeyModifiers {
                    shift: false,
                    ..
                },
                ..
            }) => {
                cx.hide_text_ime();
                cx.widget_action(uid, &scope.path, TextInputAction::Return(self.text.clone()));
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: KeyModifiers {
                    shift: true,
                    ..
                },
                ..
            }) if !self.is_read_only => {
                self.history.create_or_extend_edit_group(
                    EditKind::Other,
                    self.cursor,
                );
                self.apply_edit(Edit {
                    start: self.cursor.start().index,
                    end: self.cursor.end().index,
                    replace_with: "\n".to_string(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                cx.widget_action(uid, &scope.path, TextInputAction::Escape);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) if !self.is_read_only => {
                let mut start = self.cursor.start().index;
                let end = self.cursor.end().index;
                if start == end {
                    start = prev_grapheme_boundary(&self.text, start).unwrap_or(0);
                }
                self.history.create_or_extend_edit_group(EditKind::Backspace, self.cursor);
                self.apply_edit(Edit {
                    start,
                    end,
                    replace_with: String::new(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) if !self.is_read_only => {
                let start = self.cursor.start().index;
                let mut end = self.cursor.end().index;
                if start == end {
                    end = next_grapheme_boundary(&self.text, end).unwrap_or(self.text.len());
                }
                self.history.create_or_extend_edit_group(EditKind::Delete, self.cursor);
                self.apply_edit(Edit {
                    start,
                    end,
                    replace_with: String::new(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers: KeyModifiers {
                    control: true,
                    ..
                },
                ..
            }) | Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers: KeyModifiers {
                    logo: true,
                    ..
                },
                ..
            }) => {
                self.select_all();
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers {
                    logo: true,
                    shift: false,
                    ..
                },
                ..
            }) if !self.is_read_only => {
                self.undo();
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers {
                    logo: true,
                    shift: true,
                    ..
                },
                ..
            }) if !self.is_read_only => {
                self.redo();
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::TextInput(TextInputEvent {
                input,
                replace_last,
                was_paste,
                ..
            }) if !self.is_read_only => {
                let input = self.filter_input(input);
                if !input.is_empty() {
                    let mut start = self.cursor.start().index;
                    let end = self.cursor.end().index;
                    if replace_last {
                        start -= self.history.last_inserted_text(&self.text).map_or(0, |text| text.len());
                    }
                    self.history.create_or_extend_edit_group(
                        if replace_last || was_paste {
                            EditKind::Other
                        } else {
                            EditKind::Insert
                        },
                        self.cursor,
                    );
                    self.apply_edit(Edit {
                        start,
                        end,
                        replace_with: input,
                    });
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
                }
            }
            Hit::TextCopy(event) => {
                let selection = &self.text[self.cursor.start().index..self.cursor.end().index];
                *event.response.borrow_mut() = Some(selection.to_string());
            }
            Hit::TextCut(event) => {
                let selection = &self.text[self.cursor.start().index..self.cursor.end().index];
                *event.response.borrow_mut() = Some(selection.to_string());
                if !selection.is_empty() {
                    self.history.create_or_extend_edit_group(EditKind::Other, self.cursor);
                    self.apply_edit(Edit {
                        start: self.cursor.start().index,
                        end: self.cursor.end().index,
                        replace_with: String::new(),
                    });
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Text);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(FingerDownEvent {
                abs,
                tap_count,
                ..
            }) => {
                let event = DrawEvent::default();
                let mut cx = Cx2d::new(cx, &event);
                let index_affinity = self.position_to_index_affinity(
                    &mut cx,
                    padded_rect.size.x,
                    abs - padded_rect.pos
                );
                self.move_cursor_to(index_affinity, false);
                if tap_count == 2 {
                    self.select_word();
                }
                self.set_key_focus(&mut *cx);
                self.draw_bg.redraw(&mut *cx);
            }
            Hit::FingerMove(FingerMoveEvent {
                abs,
                tap_count,
                ..
            }) => {
                let event: DrawEvent = DrawEvent::default();
                let mut cx = Cx2d::new(cx, &event);
                let index_affinity = self.position_to_index_affinity(
                    &mut cx,
                    padded_rect.size.x,
                    abs - padded_rect.pos
                );
                self.move_cursor_to(index_affinity, true);
                if tap_count == 2 {
                    self.select_word();
                }
                self.draw_bg.redraw(&mut *cx);
            }
            _ => {}
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);

        self.draw_selection.append_to_draw_call(cx);

        // Draw text
        if self.text.is_empty() {
            self.draw_label.is_empty = 1.0;
            self.draw_label.draw_walk(
                cx,
                Walk::size(self.walk.width,self.walk.height),
                self.label_align,
                &self.empty_text,
            );
        } else {
            self.draw_label.is_empty = 0.0;
            self.draw_label.draw_walk(
                cx,
                Walk::size(self.walk.width,self.walk.height),
                self.label_align,
                &self.text,
            );
        }

        let padded_rect = cx.turtle().padded_rect_used();
     
        // Draw selection
        let rects = self.draw_label.selected_rects(
            cx,
            Walk::size(self.walk.width,self.walk.height),
            self.label_align,
            padded_rect.size.x,
            &self.text,
            self.cursor.head.min(self.cursor.tail),
            self.cursor.head.max(self.cursor.tail)
        );
        for rect in rects {
            self.draw_selection.draw_abs(cx, Rect {
                pos: padded_rect.pos + rect.pos,
                size: rect.size,
            });
        }
     
        // Draw cursor
        let cursor_position = self.cursor_position(cx, padded_rect.size.x);
        let cursor_height = self.draw_label.line_height(cx);
        self.draw_cursor.draw_abs(cx, Rect {
            pos: padded_rect.pos + dvec2(cursor_position.x - 0.5 * self.cursor_width, cursor_position.y),
            size: dvec2(self.cursor_width, cursor_height)
        });

        self.draw_bg.end(cx);

        if cx.has_key_focus(self.draw_bg.area()) {
            let padding = dvec2(self.layout.padding.left, self.layout.padding.top);
            cx.show_text_ime(
                self.draw_bg.area(), 
                padding + cursor_position - self.cursor_width * 0.5
            );
        }

        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default());

        DrawStep::done()
    }
    
    fn text(&self) -> String {
        self.text.to_string()
    }
    
    fn set_text(&mut self, text: &str) {
        self.text = self.filter_input(text.to_string());
        self.set_cursor(Cursor::default());
        self.history.clear();
    }
}

#[derive(Clone, Debug, PartialEq, DefaultNone)]
pub enum TextInputAction {
    Change(String),
    Return(String),
    Escape,
    KeyFocus,
    KeyFocusLost,
    None
}

impl TextInputRef {
    pub fn changed(&self, actions: &Actions) -> Option<String> {
        if let TextInputAction::Change(val) = actions.find_widget_action_cast(self.widget_uid()) {
            return Some(val);
        }
        None
    }

    pub fn returned(&self, actions: &Actions) -> Option<String> {
        if let TextInputAction::Return(val) = actions.find_widget_action_cast(self.widget_uid()) {
            return Some(val);
        }
        None
    }
    
    pub fn set_cursor(&self, head: usize, tail: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_cursor(Cursor {
                head: IndexAffinity {
                    index: head,
                    affinity: Affinity::After,
                },
                tail: IndexAffinity {
                    index: tail,
                    affinity: Affinity::Before,
                },
            });
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Cursor {
    pub head: IndexAffinity,
    pub tail: IndexAffinity,
}

impl Cursor {
    pub fn start(&self) -> IndexAffinity {
        self.head.min(self.tail)
    }

    pub fn end(&self) -> IndexAffinity {
        self.head.max(self.tail)
    }
}

#[derive(Clone, Debug, Default)]
struct History {
    current_edit_kind: Option<EditKind>,
    undo_stack: EditStack,
    redo_stack: EditStack,
}

impl History {
    pub fn last_inserted_text<'a>(&self, text: &'a str) -> Option<&'a str> {
        self.undo_stack.edits.last().map(|edit| &text[edit.start..edit.end])
    }

    pub fn force_new_edit_group(&mut self) {
        self.current_edit_kind = None;
    }

    pub fn create_or_extend_edit_group(
        &mut self,
        edit_kind: EditKind,
        cursor: Cursor,
    ) {
        if !self
            .current_edit_kind
            .map_or(false, |current_edit_kind| current_edit_kind.can_merge_with(edit_kind))
        {
            self.undo_stack.push_edit_group(cursor);
            self.current_edit_kind = Some(edit_kind);
        }
    }

    fn apply_edit(&mut self, edit: Edit, text: &mut String) {
        let inverted_edit = edit.invert(&text);
        edit.apply(text);
        self.undo_stack.push_edit(inverted_edit);
        self.redo_stack.clear();
    }

    pub fn undo(
        &mut self,
        cursor: Cursor,
        text: &mut String,
    ) -> Option<Cursor> {
        let mut edits = Vec::new();
        if let Some(new_cursor) = self.undo_stack.pop_edit_group(&mut edits) {
            self.redo_stack.push_edit_group(cursor);
            for edit in edits {
                let inverted_edit = edit.clone().invert(text);
                edit.apply(text);
                self.redo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_cursor)
        } else {
            None
        }
    }

    pub fn redo(
        &mut self,
        cursor: Cursor,
        text: &mut String,
    ) -> Option<Cursor> {
        let mut edits = Vec::new();
        if let Some(new_cursor) = self.redo_stack.pop_edit_group(&mut edits) {
            self.undo_stack.push_edit_group(cursor);
            for edit in edits {
                let inverted_edit = edit.clone().invert(text);
                edit.apply(text);
                self.undo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_cursor)
        } else {
            None
        }
    }

    fn clear(&mut self) {
        self.current_edit_kind = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

#[derive(Clone, Debug, Default)]
struct EditStack {
    edit_groups: Vec<EditGroup>,
    edits: Vec<Edit>,
}

impl EditStack {
    fn push_edit_group(&mut self, cursor: Cursor) {
        self.edit_groups.push(EditGroup {
            cursor,
            edit_start: self.edits.len(),
        });
    }

    fn push_edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }

    fn pop_edit_group(&mut self, edits: &mut Vec<Edit>) -> Option<Cursor> {
        match self.edit_groups.pop() {
            Some(edit_group) => {
                edits.extend(self.edits.drain(edit_group.edit_start..).rev());
                Some(edit_group.cursor)
            }
            None => None,
        }
    }

    fn clear(&mut self) {
        self.edit_groups.clear();
        self.edits.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditKind {
    Insert,
    Backspace,
    Delete,
    Other,
}

impl EditKind {
    fn can_merge_with(self, other: EditKind) -> bool {
        if self == Self::Other {
            false
        } else {
            self == other
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct EditGroup {
    cursor: Cursor,
    edit_start: usize
}

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    end: usize,
    replace_with: String,
}

impl Edit {
    fn apply(&self, text: &mut String) {
        text.replace_range(self.start..self.end, &self.replace_with);
    }

    fn invert(&self, text: &str) -> Self {
        Self {
            start: self.start,
            end: self.start + self.replace_with.len(),
            replace_with: text[self.start..self.end].to_string(),
        }
    }
}

fn next_grapheme_boundary(string: &str, index: usize) -> Option<usize> {
    let mut cursor = GraphemeCursor::new(index, string.len(), true);
    cursor.next_boundary(string, 0).unwrap()
}

fn prev_grapheme_boundary(string: &str, index: usize) -> Option<usize> {
    let mut cursor = GraphemeCursor::new(index, string.len(), true);
    cursor.prev_boundary(string, 0).unwrap()
}

/*
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

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawLabel {
    #[deref] draw_super: DrawText,
    #[live] is_empty: f32,
}

#[derive(Live, LiveHook, Widget)]
pub struct TextInput {
    #[animator] animator: Animator,
    
    #[redraw] #[live] draw_bg: DrawColor,
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
    #[live] secret: bool,
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

impl Widget for TextInput {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                cx.hide_text_ime();
                //cx.widget_action(uid, &scope.path, TextInputAction::Return(self.text.clone()));
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocusLost);
            }
            Hit::KeyFocus(_) => {
                self.undo_id += 1;
                self.animator_play(cx, id!(focus.on));
                // select all
                if self.on_focus_select_all {
                    self.select_all();
                }
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocus);
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
                if self.change(cx, &input){self.push_change_action(uid, scope, cx)}
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
                    if self.change(cx, ""){self.push_change_action(uid, scope, cx)}
                }
            }
            Hit::KeyDown(ke) => match ke.key_code {
                                
                KeyCode::Tab => {
                    // dispatch_action(cx, self, TextInputAction::Tab(key.mod_shift));
                }
                KeyCode::ReturnKey if ke.modifiers.shift => {
                    if self.change(cx, "\n"){
                        self.push_change_action(uid, scope, cx)
                    }
                },
                KeyCode::ReturnKey => {
                    cx.hide_text_ime();
                    cx.widget_action(uid, &scope.path, TextInputAction::Return(self.text.clone()));
                },
                KeyCode::Escape => {
                    cx.widget_action(uid, &scope.path, TextInputAction::Escape);
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
                    self.push_change_action(uid, scope, cx);
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
                    if let Some(pos) = self.draw_text.get_cursor_pos(cx, self.newline_indexes(), 0.0, self.cursor_head) {
                        if let Some(pos) = self.draw_text.closest_offset(cx, self.newline_indexes(), dvec2(pos.x, pos.y + self.draw_text.get_line_spacing() * 1.5)) {
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
                    if let Some(pos) = self.draw_text.get_cursor_pos(cx, self.newline_indexes(), 0.0, self.cursor_head) {
                        if let Some(pos) = self.draw_text.closest_offset(cx, self.newline_indexes(), dvec2(pos.x, pos.y - self.draw_text.get_line_spacing() * 0.5)) {
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
                    if self.change(cx, ""){self.push_change_action(uid, scope, cx)}
                }
                KeyCode::Delete => {
                    self.create_undo(UndoGroup::Delete(self.undo_id));
                    if self.cursor_head == self.cursor_tail {
                        if self.cursor_head < self.text.chars().count() {
                            self.cursor_head += 1;
                        }
                    }
                    if self.change(cx, ""){self.push_change_action(uid, scope, cx)}
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
                if let Some(pos) = self.draw_text.closest_offset(cx, self.newline_indexes(), fe.abs) {
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
                if let Some(pos) = self.draw_text.closest_offset(cx, self.newline_indexes(), fe.abs) {
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
                if let Some(pos) = self.draw_text.closest_offset(cx, self.newline_indexes(), fe.abs) {
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
                        if let Some(pos_start) = self.draw_text.closest_offset(cx, self.newline_indexes(), fe.abs_start) {
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
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk_text_input(cx, walk);
        DrawStep::done()
    }
    
    
    fn text(&self) -> String {
        self.text.clone()
    }
    
    fn set_text(&mut self, v: &str) {
        self.filter_input(&v, None);
    }
}

#[derive(Clone, Debug, PartialEq, DefaultNone)]
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
    
    pub fn set_cursor(&mut self, head:usize, tail: usize){
        self.cursor_head = head;
        self.cursor_tail = tail;
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
    
    pub fn push_change_action(&self, uid:WidgetUid, scope:&Scope, cx: &mut Cx){
        cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
    }
    
    pub fn change(&mut self, cx: &mut Cx, s: &str)->bool{
        if self.read_only {
            return false
        }
        self.replace_text(s);
        self.draw_bg.redraw(cx);
        true
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

    fn newline_indexes(&self) -> Vec<usize> {
        let mut ret = Vec::new();
        for (i, c) in self.text.chars().enumerate() {
            if c == '\n' {
                ret.push(i);
            }
        }
        ret
    }
    
    pub fn draw_walk_text_input(&mut self, cx: &mut Cx2d, walk: Walk) {
        
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
            if self.secret {
                self.draw_text.draw_walk(cx, Walk::size(
                    self.walk.width,
                    self.walk.height
                ), self.label_align, &"*".repeat(self.text.len()));
            }
            else {
                self.draw_text.draw_walk(cx, Walk::size(
                    self.walk.width,
                    self.walk.height
                ), self.label_align, &self.text);
            }
        }
        
        let mut turtle = cx.turtle().padded_rect_used();
        turtle.pos.y -= self.cursor_margin_top;
        turtle.size.y += self.cursor_margin_top + self.cursor_margin_bottom;
        // move the IME
        let line_spacing = self.draw_text.get_line_spacing();
        let top_drop = self.draw_text.get_font_size() * 0.2;
        let head = self.draw_text.get_cursor_pos(cx, self.newline_indexes(), 0.0, self.cursor_head)
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
            let rects = self.draw_text.get_selection_rects(cx, self.newline_indexes(), start, end, dvec2(0.0, -top_drop), dvec2(0.0, bottom_drop));
            for rect in rects {
                self.draw_select.draw_abs(cx, rect);
            }
        }
        self.draw_bg.end(cx);
        
        if  cx.has_key_focus(self.draw_bg.area()) {
            // ok so. if we have the IME we should inject a tracking point
            let ime_x = self.draw_text.get_cursor_pos(cx, self.newline_indexes(), 0.5, self.cursor_head)
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
*/