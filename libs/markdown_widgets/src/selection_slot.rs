//! Selection bridge for custom inline widgets. A separate handle is necessary:
//! Html is still borrowing the drawing widget when it registers the text run.
use makepad_widgets::*;
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct State {
    area: Area,
    text: String,
    start: usize,
    end: usize,
}
struct Handle {
    uid: WidgetUid,
    state: Rc<RefCell<State>>,
}
impl ScriptApply for Handle {}
impl WidgetNode for Handle {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        Walk::default()
    }
    fn area(&self) -> Area {
        self.state.borrow().area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area().redraw(cx);
    }
    fn selection_text_len(&self) -> usize {
        self.state.borrow().text.len()
    }
    fn selection_set(&mut self, a: usize, b: usize) {
        let mut s = self.state.borrow_mut();
        s.start = a.min(b);
        s.end = a.max(b);
    }
    fn selection_clear(&mut self) {
        self.selection_set(0, 0);
    }
    fn selection_select_all(&mut self) {
        self.selection_set(0, self.selection_text_len());
    }
    fn selection_get_full_text(&self) -> String {
        self.state.borrow().text.clone()
    }
    fn selection_get_text_for_range(&self, a: usize, b: usize) -> String {
        let s = self.state.borrow();
        let mut a = a.min(s.text.len());
        let mut b = b.min(s.text.len());
        while !s.text.is_char_boundary(a) {
            a -= 1;
        }
        while !s.text.is_char_boundary(b) {
            b -= 1;
        }
        s.text.get(a..b).unwrap_or("").into()
    }
}
impl Widget for Handle {}

#[derive(Default)]
pub struct SelectionSlot {
    state: Rc<RefCell<State>>,
    handle: Option<WidgetRef>,
}
impl SelectionSlot {
    pub fn register(&mut self, scope: &mut Scope, area: Area, text: &str) {
        let Some(tf) = scope.data.get_mut::<TextFlow>() else {
            return;
        };
        // This widget consumed inline space; the following word is no longer
        // at the beginning of a line, so its leading space must be retained.
        tf.first_thing_on_a_line = false;
        {
            let mut s = self.state.borrow_mut();
            s.area = area;
            if s.text != text {
                s.text = text.into();
                s.start = 0;
                s.end = 0;
            }
        }
        let handle = self.handle.get_or_insert_with(|| {
            WidgetRef::new_with_inner(Box::new(Handle {
                uid: WidgetUid::new(),
                state: self.state.clone(),
            }))
        });
        tf.push_widget_text_for_selection(handle.clone(), text);
    }
    pub fn selected(&self, start: usize, end: usize) -> bool {
        let s = self.state.borrow();
        s.start < end && s.end > start
    }
}
