use {
    crate::{
        arena::Id, buf::EditKind, edit_ops, layout, move_ops, text::Pos, text::Text, Arena, Buf,
        Diff, Event, SelSet,
    },
    std::{
        cell::{RefCell, RefMut},
        collections::HashSet,
    },
};

#[derive(Debug, Default)]
pub struct State {
    views: Arena<RefCell<View>>,
    models: Arena<Model>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_view(&mut self) -> ViewId {
        let lines: Vec<_> = include_str!("arena.rs")
            .lines()
            .map(|string| string.to_string())
            .collect();
        let line_row_counts: Vec<_> = (0..lines.len())
            .map(|line_index| layout::height(&lines[line_index]))
            .collect();
        let model = self.models.insert(Model {
            view_ids: HashSet::new(),
            buf: Buf::new(lines.into()),
        });
        let view_id = self.views.insert(RefCell::new(View {
            model_id: model,
            sels: SelSet::new(),
            line_row_counts: line_row_counts,
            line_row_indices: Vec::new(),
        }));
        self.models[model].view_ids.insert(view_id);
        ViewId(view_id)
    }

    pub fn destroy_view(&mut self, ViewId(view_id): ViewId) {
        let model_id = self.views[view_id].borrow().model_id;
        self.models[model_id].view_ids.remove(&view_id);
        if self.models[model_id].view_ids.is_empty() {
            self.models.remove(model_id);
        }
        self.views.remove(view_id);
    }

    pub fn draw(&self, ViewId(view_id): ViewId, f: impl FnOnce(DrawContext<'_>)) {
        let view = self.views[view_id].borrow();
        let model_id = view.model_id;
        f(DrawContext {
            text: &self.models[model_id].buf.text(),
            sels: &view.sels,
        });
    }

    pub fn handle_event(&mut self, ViewId(view_id): ViewId, event: Event) {
        let model_id = self.views[view_id].borrow().model_id;
        let sibling_views: Vec<_> = self.models[model_id]
            .view_ids
            .iter()
            .filter_map(|&sibling_view_id| {
                if sibling_view_id == view_id {
                    return None;
                }
                Some(self.views[sibling_view_id].borrow_mut())
            })
            .collect();
        HandleEventContext {
            view: self.views[view_id].borrow_mut(),
            sibling_views,
            model: &mut self.models[model_id],
        }
        .handle_event(event);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(Id<RefCell<View>>);

pub struct DrawContext<'a> {
    pub text: &'a Text,
    pub sels: &'a SelSet,
}

#[derive(Debug)]
struct View {
    model_id: Id<Model>,
    sels: SelSet,
    line_row_counts: Vec<usize>,
    line_row_indices: Vec<usize>,
}

impl View {
    fn line_row_index(&mut self, line_index: usize) -> usize {
        while self.line_row_indices.len() <= line_index {
            self.line_row_indices.push(if self.line_row_indices.is_empty() {
                0
            } else {
                let prev_line_index = self.line_row_indices.len() - 1;
                self.line_row_indices[prev_line_index] + self.line_row_counts[prev_line_index]
            });
        }
        self.line_row_indices[line_index]
    }

    fn update(&mut self, lines: &[String], sels: Option<SelSet>, diff: &Diff, local: bool) {
        self.update_sels(sels, diff, local);
        self.update_line_row_counts(lines, diff);
    }

    fn update_sels(&mut self, sels: Option<SelSet>, diff: &Diff, local: bool) {
        if let Some(sels) = sels {
            self.sels = sels.clone()
        } else {
            self.sels.apply_diff(diff, local);
        }
    }

    fn update_line_row_counts(&mut self, lines: &[String], diff: &Diff) {
        use crate::diff::LenOnlyOp;

        let mut line_index = 0;
        for op in diff {
            match op.len_only() {
                LenOnlyOp::Retain(len) => line_index += len.line_count,
                LenOnlyOp::Insert(len) => {
                    self.line_row_counts.splice(
                        line_index..line_index,
                        (line_index..line_index + len.line_count)
                            .map(|line_index| layout::height(&lines[line_index])),
                    );
                    line_index += len.line_count;
                    if len.byte_count > 0 {
                        self.line_row_counts[line_index] = layout::height(&lines[line_index]);
                    }
                }
                LenOnlyOp::Delete(len) => {
                    self.line_row_counts
                        .drain(line_index..line_index + len.line_count);
                    if len.byte_count > 0 {
                        self.line_row_counts[line_index] = layout::height(&lines[line_index]);
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
struct Model {
    view_ids: HashSet<Id<RefCell<View>>>,
    buf: Buf,
}

#[derive(Debug)]
struct HandleEventContext<'a> {
    view: RefMut<'a, View>,
    sibling_views: Vec<RefMut<'a, View>>,
    model: &'a mut Model,
}

impl<'a> HandleEventContext<'a> {
    fn handle_event(&mut self, event: Event) {
        use crate::event::*;

        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                self.edit(|text, sels| edit_ops::delete(text, sels));
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                self.edit(|text, sels| {
                    edit_ops::insert(
                        text,
                        sels,
                        &Text::from(vec!["".to_string(), "".to_string()]),
                    )
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Left,
            }) => {
                self.modify_sels(shift, |lines, pos, _| {
                    (move_ops::move_left(lines, pos), None)
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Up,
            }) => {
                self.modify_sels(shift, |lines, pos, col_index| {
                    move_ops::move_up(lines, pos, col_index)
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Right,
            }) => {
                self.modify_sels(shift, |lines, pos, _| {
                    (move_ops::move_right(lines, pos), None)
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Down,
            }) => {
                self.modify_sels(shift, |lines, pos, col_index| {
                    move_ops::move_down(lines, pos, col_index)
                });
            }
            Event::Key(KeyEvent {
                modifiers:
                    KeyModifiers {
                        command: true,
                        shift: false,
                    },
                code: KeyCode::Z,
            }) => {
                self.undo();
            }
            Event::Key(KeyEvent {
                modifiers:
                    KeyModifiers {
                        command: true,
                        shift: true,
                    },
                code: KeyCode::Z,
            }) => {
                self.redo();
            }
            Event::Text(TextEvent { string }) => {
                self.edit(|text, sels| {
                    edit_ops::insert(
                        text,
                        sels,
                        &string
                            .lines()
                            .map(|string| string.to_string())
                            .collect::<Text>(),
                    )
                });
            }
            _ => {}
        }
    }

    fn modify_sels(
        &mut self,
        select: bool,
        mut f: impl FnMut(&[String], Pos, Option<usize>) -> (Pos, Option<usize>),
    ) {
        self.view.sels.modify_all(|sel| {
            let mut sel = sel.modify_cursor_pos(|pos, col_index| {
                f(self.model.buf.text().as_lines(), pos, col_index)
            });
            if !select {
                sel = sel.reset_anchor_pos();
            }
            sel
        });
        self.model.buf.flush();
    }

    fn edit(&mut self, f: impl FnOnce(&Text, &SelSet) -> (EditKind, Diff)) {
        let (kind, diff) = f(self.model.buf.text(), &self.view.sels);
        self.model.buf.edit(kind, &self.view.sels, diff.clone());
        self.update_views(None, &diff);
    }

    fn undo(&mut self) {
        if let Some((sels, diff)) = self.model.buf.undo() {
            self.update_views(Some(sels), &diff);
        }
    }

    fn redo(&mut self) {
        if let Some((sels, diff)) = self.model.buf.redo() {
            self.update_views(Some(sels), &diff);
        }
    }

    fn update_views(&mut self, sels: Option<SelSet>, diff: &Diff) {
        let lines = self.model.buf.text().as_lines();
        self.view.update(lines, sels, diff, true);
        for sibling_view in &mut self.sibling_views {
            sibling_view.update(lines, None, diff, false);
        }
    }
}
