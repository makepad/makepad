use {
    crate::{
        arena::Id, buf::EditKind, edit_ops, move_ops, text::Pos, text::Text, Arena, Buf, Diff,
        Event, SelSet,
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
        let text: Text = include_str!("arena.rs")
            .lines()
            .map(|string| string.to_string())
            .collect();
        let model = self.models.insert(Model {
            view_ids: HashSet::new(),
            buf: Buf::new(text),
        });
        let view_id = self.views.insert(RefCell::new(View {
            model_id: model,
            sels: SelSet::new(),
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
}

impl View {
    fn update(&mut self, sels: Option<&SelSet>, diff: &Diff, local: bool) {
        if let Some(sels) = sels {
            self.sels = sels.clone()
        } else {
            self.sels.apply_diff(diff, local);
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
                self.edit(|context| edit_ops::delete(context));
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                self.edit(|context| {
                    edit_ops::insert(context, &Text::from(vec!["".to_string(), "".to_string()]))
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Left,
            }) => {
                self.update_cursors(shift, |context, pos, _| {
                    (move_ops::move_left(context, pos), None)
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Up,
            }) => {
                self.update_cursors(shift, |context, pos, column| {
                    move_ops::move_up(context, pos, column)
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Right,
            }) => {
                self.update_cursors(shift, |context, pos, _| {
                    (move_ops::move_right(context, pos), None)
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Down,
            }) => {
                self.update_cursors(shift, |context, pos, column| {
                    move_ops::move_down(context, pos, column)
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
                self.edit(|context| {
                    edit_ops::insert(
                        context,
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

    fn update_cursors(
        &mut self,
        select: bool,
        mut f: impl FnMut(&move_ops::Context<'_>, Pos, Option<usize>) -> (Pos, Option<usize>),
    ) {
        self.view.sels.update_all(|sel| {
            let mut sel = sel.update_cursor(|pos, column| {
                f(
                    &move_ops::Context {
                        lines: self.model.buf.text().as_lines(),
                    },
                    pos,
                    column,
                )
            });
            if !select {
                sel = sel.reset_anchor();
            }
            sel
        });
        self.model.buf.flush();
    }

    fn edit(&mut self, f: impl FnOnce(&edit_ops::Context<'_>) -> (EditKind, Diff)) {
        let (kind, diff) = f(&edit_ops::Context {
            sels: &self.view.sels,
            text: self.model.buf.text(),
        });
        self.model.buf.edit(kind, &self.view.sels, diff.clone());
        self.update_views(None, &diff);
    }

    fn undo(&mut self) {
        if let Some((sels, diff)) = self.model.buf.undo() {
            self.update_views(Some(&sels), &diff);
        }
    }

    fn redo(&mut self) {
        if let Some((sels, diff)) = self.model.buf.redo() {
            self.update_views(Some(&sels), &diff);
        }
    }

    fn update_views(&mut self, sels: Option<&SelSet>, diff: &Diff) {
        self.view.update(sels, diff, true);
        for sibling_view in &mut self.sibling_views {
            sibling_view.update(None, diff, false);
        }
    }
}
