use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::{text::{selection::{Cursor, Selection}, layout::LaidoutText}, *},
        widget::*,
    },
    std::rc::Rc,
};


live_design! {
    link widgets;

    use link::theme::*;
    use makepad_draw::shader::std::*;

    pub TextInput2Base = {{TextInput2}} {}
    
    pub TextInput2 = <TextInput2Base> {
        width: 200,
        height: Fit,

        draw_text: {
            instance hover: 0.0
            instance focus: 0.0
            wrap: Word,
            text_style: {
                font_family: <THEME_FONT_FAMILY_REGULAR> {},
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: 16.0
            }
        }

        draw_selection: {
            instance hover: 0.0
            instance focus: 1.0 // TODO: Animate this
            
            uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.fill(mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_BG_HIGHLIGHT_INLINE, self.focus));
                return sdf.result
            }
        }

        draw_cursor: {
            instance focus: 1.0 // TODO: Animate this
            uniform border_radius: 0.5

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.fill(mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_TEXT_CURSOR, self.focus));
                return sdf.result
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct TextInput2 {
    #[redraw] #[live] draw_bg: DrawColor,
    #[live] draw_text: DrawText2,
    #[live] draw_selection: DrawQuad,
    #[live] draw_cursor: DrawQuad,

    #[layout] layout: Layout,
    #[walk] text_walk: Walk,
    #[live] text_align: Align,

    #[live] pub text: String,
    #[rust] laidout_text: Option<Rc<LaidoutText>>,
    #[rust] text_area: Area,
    #[rust] selection: Selection,
    #[rust] history: History,
}

impl TextInput2 {
    fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.draw_bg.area());
    }

    fn selected_text(&self) -> &str {
        &self.text[self.selection.start().index..self.selection.end().index]
    }

    fn move_cursor_left(&mut self, keep_selection: bool) {
        self.set_cursor(
            Cursor {
                index: prev_grapheme_boundary(&self.text, self.selection.cursor.index),
                prefer_next_row: true,
            },
            keep_selection
        );
    }

    fn move_cursor_right(&mut self, keep_selection: bool) {
        self.set_cursor(
            Cursor {
                index: next_grapheme_boundary(&self.text, self.selection.cursor.index),
                prefer_next_row: false,
            },
            keep_selection,
        );
    }

    fn move_cursor_up(&mut self, keep_selection: bool) {
        use makepad_draw::text::selection::Position;

        let laidout_text = self.laidout_text.as_ref().unwrap();
        let position = laidout_text.cursor_to_position(self.selection.cursor);
        self.set_cursor(
            laidout_text.position_to_cursor(Position {
                row_index: if position.row_index == 0 {
                    0
                } else {
                    position.row_index - 1
                },
                x_in_lpxs: position.x_in_lpxs,
            }),
            keep_selection
        );
    }

    fn move_cursor_down(&mut self, keep_selection: bool) {
        use makepad_draw::text::selection::Position;
        
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let position = laidout_text.cursor_to_position(self.selection.cursor);
        self.set_cursor(
            laidout_text.position_to_cursor(Position {
                row_index: if position.row_index == laidout_text.rows.len() - 1 {
                    laidout_text.rows.len() - 1
                } else {
                    position.row_index + 1 
                },
                x_in_lpxs: position.x_in_lpxs,
            }),
            keep_selection
        );
    }

    fn set_cursor(&mut self, cursor: Cursor, keep_selection: bool) {
        self.selection.cursor = cursor;
        if !keep_selection {
            self.selection.anchor = cursor;
        }
        self.history.force_new_edit_group();
    }

    fn create_or_extend_edit_group(&mut self, edit_kind: EditKind) {
        self.history.create_or_extend_edit_group(edit_kind, self.selection);
    }

    fn apply_edit(&mut self, edit: Edit) {
        self.selection.cursor.index = edit.start + edit.replace_with.len();
        self.selection.anchor.index = self.selection.cursor.index;
        self.history.apply_edit(edit, &mut self.text);
        self.laidout_text = None;
    }

    fn undo(&mut self) -> bool {
        if let Some(new_selection) = self.history.undo(self.selection, &mut self.text) {
            self.laidout_text = None;
            self.selection = new_selection;
            true
        } else {
            false
        }
    }

    fn redo(&mut self) -> bool {
        if let Some(new_selection) = self.history.redo(self.selection, &mut self.text) {
            self.laidout_text = None;
            self.selection = new_selection;
            true
        } else {
            false
        }
    }
}

impl Widget for TextInput2 {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        use makepad_draw::text::selection::Position;
        
        self.draw_bg.begin(cx, walk, self.layout);

        if self.laidout_text.is_none() {
            self.laidout_text = Some(self.draw_text.layout(cx, self.text_walk, self.text_align, &self.text));
        }
        let laidout_text = self.laidout_text.as_ref().unwrap();

        let text_rect = self.draw_text.draw_walk_laidout(
            cx,
            self.text_walk,
            self.text_align,
            laidout_text,
        );
        cx.add_aligned_rect_area(&mut self.text_area, text_rect);

        let Position {
            row_index,
            x_in_lpxs,
        } = laidout_text.cursor_to_position(self.selection.cursor);
        let row = &laidout_text.rows[row_index];
        self.draw_cursor.draw_abs(
            cx,
            rect(
                text_rect.pos.x + x_in_lpxs as f64 - 2.0 / 2.0,
                text_rect.pos.y + (row.origin_in_lpxs.y - row.ascender_in_lpxs) as f64,
                2.0,
                (row.ascender_in_lpxs - row.descender_in_lpxs) as f64,
            )
        );

        for rect_in_lpxs in laidout_text.selection_rects_in_lpxs(self.selection) {
            self.draw_selection.draw_abs(
                cx,
                rect(
                    text_rect.pos.x + rect_in_lpxs.origin.x as f64,
                    text_rect.pos.y + rect_in_lpxs.origin.y as f64,
                    rect_in_lpxs.size.width as f64,
                    rect_in_lpxs.size.height as f64,
                )
            );
        }

        self.draw_bg.end(cx);

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        use makepad_draw::text::geom::Point;

        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_left(keep_selection);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_right(keep_selection);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_up(keep_selection);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_down(keep_selection);
                self.draw_bg.redraw(cx);
            },
            Hit::FingerDown(FingerDownEvent {
                abs,
                device,
                ..
            }) if device.is_primary_hit() => {
                self.set_key_focus(cx);
                let laidout_text = self.laidout_text.as_ref().unwrap();
                let rel = abs - self.text_area.rect(cx).pos;
                self.set_cursor(laidout_text.point_in_lpxs_to_cursor(
                    Point::new(rel.x as f32, rel.y as f32)
                ), false);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(FingerMoveEvent {
                abs,
                device,
                ..
            }) if device.is_primary_hit() => {
                self.set_key_focus(cx);
                let laidout_text = self.laidout_text.as_ref().unwrap();
                let rel = abs - self.text_area.rect(cx).pos;
                self.set_cursor(laidout_text.point_in_lpxs_to_cursor(
                    Point::new(rel.x as f32, rel.y as f32)
                ), true);
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                let mut start = self.selection.start().index;
                let end = self.selection.end().index;
                if start == end {
                    start = prev_grapheme_boundary(&self.text, start);
                }
                self.create_or_extend_edit_group(EditKind::Backspace);
                self.apply_edit(Edit {
                    start,
                    end,
                    replace_with: String::new(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInput2Action::Changed(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                let start = self.selection.start().index;
                let mut end = self.selection.end().index;
                if start == end {
                    end = next_grapheme_boundary(&self.text, end);
                }
                self.create_or_extend_edit_group(EditKind::Delete);
                self.apply_edit(Edit {
                    start,
                    end,
                    replace_with: String::new(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInput2Action::Changed(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: KeyModifiers {
                    shift: true,
                    ..
                },
                ..
            }) => {
                self.create_or_extend_edit_group(EditKind::Other);
                self.apply_edit(Edit {
                    start: self.selection.start().index,
                    end: self.selection.end().index,
                    replace_with: "\n".to_string(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInput2Action::Changed(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: modifiers @ KeyModifiers {
                    shift: false,
                    ..
                },
                ..
            }) if modifiers.is_primary() => {
                if self.undo() {
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, &scope.path, TextInput2Action::Changed(self.text.clone()));
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: modifiers @ KeyModifiers {
                    shift: true,
                    ..
                },
                ..
            }) if modifiers.is_primary() => {
                if self.redo() {
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, &scope.path, TextInput2Action::Changed(self.text.clone()));
                }
            }
            Hit::TextInput(TextInputEvent {
                input,
                replace_last,
                was_paste,
                ..
            }) if !input.is_empty() => {
                self.create_or_extend_edit_group(
                    if replace_last || was_paste {
                        EditKind::Other
                    } else {
                        EditKind::Insert
                    }
                );
                self.apply_edit(Edit {
                    start: self.selection.start().index,
                    end: self.selection.end().index,
                    replace_with: input
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInput2Action::Changed(self.text.clone()));
            }
            Hit::TextCopy(event) => {
                *event.response.borrow_mut() = Some(self.selected_text().to_string());
            }
            _ => {}
        }
    }
}

impl TextInput2Ref {
    pub fn editd(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TextInput2Action>(self.widget_uid()){
            if let TextInput2Action::Changed(val) = action{
                return Some(val);
            }
        }
        None
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum TextInput2Action {
    None,
    Changed(String),
}

#[derive(Clone, Debug, Default)]
struct History {
    current_edit_kind: Option<EditKind>,
    undo_stack: EditStack,
    redo_stack: EditStack,
}

impl History {
    fn force_new_edit_group(&mut self, selection: Selection) {
        self.current_edit_kind = None;
    }

    fn create_or_extend_edit_group(&mut self, edit_kind: EditKind, selection: Selection) {
        if !self.current_edit_kind.map_or(false, |current_edit_kind| current_edit_kind.can_merge_with(edit_kind)) {
            self.undo_stack.push_edit_group(selection);
            self.current_edit_kind = Some(edit_kind);
        }
    }

    fn apply_edit(&mut self, edit: Edit, text: &mut String) {
        let inverted_edit = edit.invert(&text);
        edit.apply(text);
        self.undo_stack.push_edit(inverted_edit);
        self.redo_stack.clear();
    }

    fn undo(
        &mut self,
        selection: Selection,
        text: &mut String,
    ) -> Option<Selection> {
        if let Some((new_selection, edits)) = self.undo_stack.pop_edit_group() {
            self.redo_stack.push_edit_group(selection);
            for edit in &edits {
                let inverted_edit = edit.invert(text);
                edit.apply(text);
                self.redo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_selection)
        } else {
            None
        }
    }

    fn redo(
        &mut self,
        selection: Selection,
        text: &mut String,
    ) -> Option<Selection> {
        if let Some((new_selection, edits)) = self.redo_stack.pop_edit_group() {
            self.undo_stack.push_edit_group(selection);
            for edit in &edits {
                let inverted_edit = edit.invert(text);
                edit.apply(text);
                self.undo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_selection)
        } else {
            None
        }
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

#[derive(Clone, Debug, Default)]
struct EditStack {
    edit_groups: Vec<EditGroup>,
    edits: Vec<Edit>,
}

impl EditStack {
    fn push_edit_group(&mut self, selection: Selection) {
        self.edit_groups.push(EditGroup {
            selection,
            edit_start: self.edits.len(),
        });
    }
    
    fn push_edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }
    
    fn pop_edit_group(&mut self) -> Option<(Selection, Vec<Edit>)> {
        match self.edit_groups.pop() {
            Some(edit_group) => Some((
                edit_group.selection,
                self.edits.drain(edit_group.edit_start..).rev().collect()
            )),
            None => None,
        }
    }
    
    fn clear(&mut self) {
        self.edit_groups.clear();
        self.edits.clear();
    }
}

#[derive(Clone, Copy, Debug)]
struct EditGroup {
    selection: Selection,
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

fn prev_grapheme_boundary(text: &str, index: usize) -> usize {
    use unicode_segmentation::GraphemeCursor;

    let mut cursor = GraphemeCursor::new(index, text.len(), true);
    cursor.prev_boundary(text, 0).unwrap().unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, index: usize) -> usize {
    use unicode_segmentation::GraphemeCursor;

    let mut cursor = GraphemeCursor::new(index, text.len(), true);
    cursor.next_boundary(text, 0).unwrap().unwrap_or(text.len())
}