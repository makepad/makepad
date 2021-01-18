use {
    makepad_code_editor::{
        code_editor::{self, CodeEditor},
        text::Text,
    },
    makepad_render::*,
    makepad_widget::*,
};

pub struct App {
    window: DesktopWindow,
    editor: CodeEditor,
    text: Text,
}

impl App {
    pub fn new(cx: &mut Cx) -> App {
        App {
            window: DesktopWindow::new(cx),
            editor: CodeEditor::new(cx),
            text: r#"Moge je vader je moeder wezen"#.parse().unwrap(),
        }
    }

    pub fn style(cx: &mut Cx) {
        set_widget_style(cx);
        code_editor::set_code_editor_style(cx);
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_desktop_window(cx, event);
        self.editor.handle_code_editor(cx, event);
    }

    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.window.begin_desktop_window(cx, None).is_err() {
            return;
        };

        self.editor.draw_code_editor(cx, self.text.as_lines());

        self.window.end_desktop_window(cx);
    }
}

fn main() {
    main_app!(App);
}
