use {
    crate::{
        makepad_derive_widget::*,
        widget::*,
        makepad_draw::*,
        button::{Button}
    }
};

live_design!{
    LinkLabelBase = {{LinkLabel}} {}
}

#[derive(Live)]
pub struct LinkLabel {
    #[deref] button: Button
}

impl LiveHook for LinkLabel {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, LinkLabel)
    }
}

impl Widget for LinkLabel {
    fn redraw(&mut self, cx: &mut Cx) {
        self.button.redraw(cx)
    }
    
    fn walk(&self) -> Walk {
        self.button.walk()
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.button.draw_walk_widget(cx, walk)
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct LinkLabelRef(WidgetRef);

impl LinkLabelRef {
    pub fn set_text(&self, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            let s = inner.button.text.as_mut_empty();
            s.push_str(text);
        }
    }
}
