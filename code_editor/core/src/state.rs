use {
    crate::{arena::Id, mv, sel::Region, Arena, Buf, Diff, Event, Sel, Text},
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
            views: HashSet::new(),
            buf: Buf::new(),
        });
        let view = self.views.insert(RefCell::new(View {
            model,
            sel: Sel::new(),
        }));
        self.models[model].views.insert(view);
        ViewId(view)
    }

    pub fn destroy_view(&mut self, ViewId(view): ViewId) {
        let model = self.views[view].borrow().model;
        self.models[model].views.remove(&view);
        if self.models[model].views.is_empty() {
            self.models.remove(model);
        }
        self.views.remove(view);
    }

    pub fn handle_event(&mut self, ViewId(view): ViewId, event: Event) {
        let model = self.views[view].borrow().model;
        let sibling_views: Vec<_> = self.models[model]
            .views
            .iter()
            .filter_map(|&sibling_view| {
                if sibling_view == view {
                    return None;
                }
                Some(self.views[sibling_view].borrow_mut())
            })
            .collect();
        HandleEventContext {
            view: self.views[view].borrow_mut(),
            sibling_views,
            model: &mut self.models[model],
        }
        .handle_event(event);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(Id<RefCell<View>>);

#[derive(Debug)]
struct View {
    model: Id<Model>,
    sel: Sel,
}

impl View {
    fn move_sel(&mut self, text: &Text, mut f: impl FnMut(&mv::Context<'_>, &mut Region)) {
        let context = mv::Context { text };
        self.sel.update_all(|region| f(&context, region));
    }

    fn apply_diff(&mut self, diff: &Diff, local: bool) {
        self.sel.apply_diff(diff, local);
    }
}

#[derive(Debug)]
struct Model {
    views: HashSet<Id<RefCell<View>>>,
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
        use crate::{edit, event::*, Event::*};

        match event {
            Key(KeyEvent {
                modifiers: KeyModifiers { shift },
                code: KeyCode::Left,
            }) => {
                self.view
                    .move_sel(self.model.buf.text(), |context, region| {
                        region.update(|pos, _| (context.move_left(pos), None));
                        if !shift {
                            region.clear();
                        }
                    });
            }
            Key(KeyEvent {
                modifiers: KeyModifiers { shift },
                code: KeyCode::Right,
            }) => {
                self.view
                    .move_sel(self.model.buf.text(), |context, region| {
                        region.update(|pos, _| (context.move_right(pos), None));
                        if !shift {
                            region.clear();
                        }
                    });
            }
            Key(KeyEvent {
                modifiers: KeyModifiers { shift },
                code: KeyCode::Up,
            }) => {
                self.view
                    .move_sel(self.model.buf.text(), |context, region| {
                        region.update(|pos, column| context.move_up(pos, column));
                        if !shift {
                            region.clear();
                        }
                    });
            }
            Key(KeyEvent {
                modifiers: KeyModifiers { shift },
                code: KeyCode::Down,
            }) => {
                self.view
                    .move_sel(self.model.buf.text(), |context, region| {
                        region.update(|pos, column| context.move_down(pos, column));
                        if !shift {
                            region.clear();
                        }
                    });
            }
            Text(TextEvent { string }) => {
                let diff = edit::Context {
                    sel: &self.view.sel,
                }
                .insert(string.parse().unwrap());
                self.model.buf.apply_diff(diff.clone());
                self.view.apply_diff(&diff, true);
                for sibling_view in &mut self.sibling_views {
                    sibling_view.apply_diff(&diff, false);
                }
            }
        }
    }
}
