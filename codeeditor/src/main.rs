use {
    makepad_code_editor::{
        components::{self, Root},
        State,
    },
    makepad_render::*,
    makepad_widget::*,
};

pub struct App {
    root: Root,
    state: State,
}

impl App {
    pub fn style(cx: &mut Cx) {
        set_widget_style(cx);
        components::style(cx);
    }

    pub fn new(cx: &mut Cx) -> App {
        App {
            root: Root::new(cx),
            state: State::new(),
        }
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        if let Some(action) = self.root.handle(cx, event, &self.state) {
            self.state.handle_action(action);
            self.handle_app(cx, &mut Event::Redraw);
        }
    }

    pub fn draw_app(&mut self, cx: &mut Cx) {
        self.root.draw(cx, &self.state);
    }
}

fn main() {
    main_app!(App);
}
