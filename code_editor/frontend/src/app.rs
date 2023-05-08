use {makepad_widgets::*, makepad_code_editor_core::state::ViewId};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;

    App = {{App}} {
        ui: <DesktopWindow> {
            editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(editor)) {
                    // self.editor.draw_editor(cx);
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
    }
}

struct State {
    state: makepad_code_editor_core::State,
    view: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut state = makepad_code_editor_core::State::new();
        let view = state.create_view();
        Self {
            state: makepad_code_editor_core::State::new(),
            view,
        }
    }
}

app_main!(App);