use {
    crate::{
        components::{code_editor, CodeEditor},
        State,
    },
    makepad_render::*,
    makepad_widget::*,
};

pub struct Root {
    window: DesktopWindow,
    editor: CodeEditor,
}

impl Root {
    pub fn style(_cx: &mut Cx) {}

    pub fn new(cx: &mut Cx) -> Root {
        Root {
            window: DesktopWindow::new(cx),
            editor: CodeEditor::new(cx),
        }
    }

    pub fn handle(&mut self, cx: &mut Cx, event: &mut Event, state: &State) -> Option<Action> {
        self.window.handle_desktop_window(cx, event);
        let editor = &state.editors[state.editor_index];
        self.editor
            .handle(
                cx,
                event,
                &state.documents[editor.document_index].text,
                &editor.selections,
                &editor.carets,
            )
            .map(|action| Action(action))
    }

    pub fn draw(&mut self, cx: &mut Cx, state: &State) {
        if self.window.begin_desktop_window(cx, None).is_err() {
            return;
        }
        let editor = &state.editors[state.editor_index];
        self.editor.draw(
            cx,
            &state.documents[editor.document_index].text,
            &editor.selections,
            &editor.carets,
        );
        self.window.end_desktop_window(cx);
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Action(pub code_editor::Action);
