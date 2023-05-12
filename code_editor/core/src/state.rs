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
    fn apply_diff(&mut self, diff: &Diff, local: bool) {
        self.cursors.apply_diff(diff, local);
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
                modifiers: KeyModifiers { shift },
                code: KeyCode::Left,
            }) => {
                self.view.cursors.update_all(|region| {
                    let mut region = region
                        .apply_move(|pos, _| (mv::move_left(self.model.buf.text(), pos), None));
                    if !shift {
                        region = region.clear();
                    }
                    region
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift },
                code: KeyCode::Right,
            }) => {
                self.view.cursors.update_all(|region| {
                    let mut region = region
                        .apply_move(|pos, _| (mv::move_right(self.model.buf.text(), pos), None));
                    if !shift {
                        region = region.clear();
                    }
                    region
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift },
                code: KeyCode::Up,
            }) => {
                self.view.cursors.update_all(|region| {
                    let mut region = region
                        .apply_move(|pos, column| mv::move_up(self.model.buf.text(), pos, column));
                    if !shift {
                        region = region.clear();
                    }
                    region
                });
            }
            Event::Key(KeyEvent {
                modifiers: KeyModifiers { shift },
                code: KeyCode::Down,
            }) => {
                self.view.cursors.update_all(|region| {
                    let mut region = region.apply_move(|pos, column| {
                        mv::move_down(self.model.buf.text(), pos, column)
                    });
                    if !shift {
                        region = region.clear();
                    }
                    region
                });
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                let context = edit::Context {
                    text: self.model.buf.text(),
                };
                let replace_with = ["".to_string(), "".to_string()].into();
                let diff = context.insert(self.view.cursors.spans(), &replace_with);
                self.model.buf.apply_diff(diff.clone());
                self.view.apply_diff(&diff, true);
                for sibling_view in &mut self.sibling_views {
                    sibling_view.apply_diff(&diff, false);
                }
            }
            Event::Text(TextEvent { string }) => {
                let context = edit::Context {
                    text: self.model.buf.text(),
                };
                let replace_with = string.into();
                let diff = context.insert(self.view.cursors.spans(), &replace_with);
                self.model.buf.apply_diff(diff.clone());
                self.view.apply_diff(&diff, true);
                for sibling_view in &mut self.sibling_views {
                    sibling_view.apply_diff(&diff, false);
                }
            }
        }
    }
}
