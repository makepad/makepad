use {
    crate::{code_editor, code_editor::CodeEditor},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    
    App = {{App}} {
        ui: <DesktopWindow> {frame:{body={user_draw:true}}}
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

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            if self.ui.draw_widget_continue(&mut cx).is_not_done(){
                self.editor.draw(&mut cx);
                self.ui.draw_widget(&mut cx);
            }
            return
        }
        self.ui.handle_widget_event(cx, event);
        self.editor.handle_event(cx, event);
    }
}

struct AppState;

impl AppState {
    pub fn new(cx: &mut Cx) -> Self {
        Self
    }
}

app_main!(App);