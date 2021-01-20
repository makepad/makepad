use {
    makepad_code_editor::{CodeEditor, Document},
    makepad_render::*,
    makepad_widget::*,
};

pub struct App {
    window: DesktopWindow,
    editor: CodeEditor,
    document: Document,
}

impl App {
    pub fn new(cx: &mut Cx) -> App {
        App {
            window: DesktopWindow::new(cx),
            editor: CodeEditor::new(cx),
            document: Document::from(
                include_str!("tokenizer.rs")
                    .lines()
                    .map(|line| line.chars().collect::<Vec<_>>())
                    .collect::<Document>(),
            ),
        }
    }

    pub fn style(cx: &mut Cx) {
        set_widget_style(cx);
        makepad_code_editor::set_code_editor_style(cx);
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_desktop_window(cx, event);
        self.editor.handle_code_editor(cx, event);
    }

    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.window.begin_desktop_window(cx, None).is_err() {
            return;
        };
        self.editor.draw_code_editor(cx, &self.document);
        self.window.end_desktop_window(cx);
    }
}

fn main() {
    main_app!(App);
}
