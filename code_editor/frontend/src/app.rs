use {
    crate::{code_editor, code_editor::CodeEditor},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    
    App = {{App}} {
        ui: <DesktopWindow> {}
    }
}

#[derive(Live, LiveHook)]
#[live_design_with {
    makepad_widgets::live_design(cx);
    code_editor::live_design(cx);
}]
pub struct App {
    ui: WidgetRef,
    editor: CodeEditor,
    #[rust(AppState::new(cx))]
    app_state: AppState,
}

impl App {
    pub fn draw(&mut self, cx: &mut Cx2d) {
        /*
        if self.ui.begin(cx).is_redrawing() {
            self.editor.draw(cx);
            self.ui.end(cx);
        }
        */
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_widget_event(cx, event);
        self.editor.handle_event(cx, event);
        match event {
            Event::Draw(event) => {
                let mut cx = Cx2d::new(cx, event);
                self.draw(&mut cx)
            },
            _ => {}
        }
    }
}

struct AppState;

impl AppState {
    pub fn new(cx: &mut Cx) -> Self {
        Self
    }
}

app_main!(App);