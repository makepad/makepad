use {
    makepad_code_editor::{
        code_editor,
        state::{Document, Session},
        CodeEditor,
    },
    makepad_widgets::*,
    std::{cell::RefCell, rc::Rc},
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::hook_widget::HookWidget;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    code_editor: CodeEditor,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(code_editor)) {
                    self.code_editor.draw(&mut cx, &mut self.state.session);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        self.code_editor
            .handle_event(cx, event, &mut self.state.session);
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    session: Session,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: Session::new(Rc::new(RefCell::new(Document::new(
                include_str!("state.rs").into(),
            )))),
        }
    }
}

app_main!(App);
