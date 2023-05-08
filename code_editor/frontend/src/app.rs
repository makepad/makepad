use {
    crate::{code_editor, CodeEditor},
    makepad_code_editor_core::state::ViewId,
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::hook_widget::HookWidget;

    App = {{App}} {
        ui: <DesktopWindow> {
            editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    editor: CodeEditor,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(editor)) {
                    self.editor
                        .draw(&mut cx, &self.state.editor, self.state.view);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        self.editor.handle_event(cx, event);
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    editor: makepad_code_editor_core::State,
    view: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut editor = makepad_code_editor_core::State::new();
        let view = editor.create_view();
        Self {
            editor,
            view,
        }
    }
}

app_main!(App);
