use {
    crate::{arena::Id, mv, Arena, Buf, CursorSet, Diff, Event, Text},
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
        let model = self.models.insert(Model {
            view_ids: HashSet::new(),
            buf: Buf::new(include_str!("arena.rs").into()),
        });
        let view_id = self.views.insert(RefCell::new(View {
            model_id: model,
            cursors: CursorSet::new(),
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

    pub fn draw(&self, ViewId(view_id): ViewId, f: impl FnOnce(&Text, &CursorSet)) {
        let model_id = self.views[view_id].borrow().model_id;
        f(
            &self.models[model_id].buf.text(),
            &self.views[view_id].borrow().cursors,
        );
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

#[derive(Debug)]
struct View {
    model_id: Id<Model>,
    cursors: CursorSet,
}

impl View {
    fn update(&mut self, cursors: Option<CursorSet>, diff: &Diff, local: bool) {
        if let Some(cursors) = cursors {
            self.cursors = cursors
        } else {
            self.cursors.apply_diff(diff, local);
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
        use crate::{edit, event::*};

        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                self.edit(edit::delete(self.model.buf.text(), &self.view.cursors));
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                let replace_with = ["".to_string(), "".to_string()].into();
                self.edit(edit::insert(
                    self.model.buf.text(),
                    &self.view.cursors,
                    &replace_with,
                ));
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Left,
            }) => {
                self.view.cursors.update_all(|region| {
                    let mut region = region
                        .apply_motion(|pos, _| (mv::move_left(self.model.buf.text(), pos), None));
                    if !shift {
                        region = region.clear();
                    }
                    region
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Up,
            }) => {
                self.view.cursors.update_all(|region| {
                    let mut region = region.apply_motion(|pos, column| {
                        mv::move_up(self.model.buf.text(), pos, column)
                    });
                    if !shift {
                        region = region.clear();
                    }
                    region
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Right,
            }) => {
                self.view.cursors.update_all(|region| {
                    let mut region = region
                        .apply_motion(|pos, _| (mv::move_right(self.model.buf.text(), pos), None));
                    if !shift {
                        region = region.clear();
                    }
                    region
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift, .. },
                code: KeyCode::Down,
            }) => {
                self.view.cursors.update_all(|region| {
                    let mut region = region.apply_motion(|pos, column| {
                        mv::move_down(self.model.buf.text(), pos, column)
                    });
                    if !shift {
                        region = region.clear();
                    }
                    region
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
                let replace_with = string.into();
                self.edit(edit::insert(
                    self.model.buf.text(),
                    self.view.cursors.iter(),
                    &replace_with,
                ));
            }
            _ => {}
        }
    }

    fn edit(&mut self, diff: Diff) {
        self.model.buf.begin_commit(self.view.cursors.clone());
        self.model.buf.apply_diff(diff.clone());
        self.model.buf.end_commit();
        self.update_views(None, &diff);
    }

    fn undo(&mut self) {
        if let Some((cursors_before, diff)) = self.model.buf.undo() {
            self.update_views(Some(cursors_before), &diff);
        }
    }

    fn redo(&mut self) {
        if let Some((diff, cursors_after)) = self.model.buf.redo() {
            self.update_views(Some(cursors_after), &diff);
        }
    }

    fn update_views(&mut self, cursors: Option<CursorSet>, diff: &Diff) {
        self.view.update(cursors, diff, true);
        for sibling_view in &mut self.sibling_views {
            sibling_view.update(None, diff, false);
        }
    }
}
