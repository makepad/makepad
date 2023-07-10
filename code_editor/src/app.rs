use {
    makepad_code_editor::{code_editor, state::ViewId, CodeEditor},
    makepad_widgets::*,
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
                    self.code_editor
                        .draw(&mut cx, &mut self.state.code_editor, self.state.view_id);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    code_editor: makepad_code_editor::State,
    view_id: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut code_editor = makepad_code_editor::State::new();
        let view_id = code_editor.open_view("code_editor/src/line.rs").unwrap();
        Self {
            code_editor,
            view_id,
        }
    }
}

app_main!(App);
