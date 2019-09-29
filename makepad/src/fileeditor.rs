//use syn::Type;
use render::*;
use editor::*;

#[derive(Clone)]
pub struct FileEditorTemplates {
    pub rust_editor: RustEditor,
    pub js_editor: JSEditor,
    //text_editor: TextEditor
}

#[derive(Clone)]
pub enum FileEditor {
    Rust(RustEditor),
    JS(JSEditor),
    //Text(TextEditor)
}

#[derive(Clone)]
pub enum FileEditorEvent {
    None,
    LagChange,
    Change
}

impl FileEditor {
    pub fn handle_file_editor(&mut self, cx: &mut Cx, event: &mut Event, text_buffer: &mut TextBuffer) -> FileEditorEvent {
        match self {
            FileEditor::Rust(re) => {
                match re.handle_rust_editor(cx, event, text_buffer) {
                    CodeEditorEvent::Change => FileEditorEvent::Change,
                    CodeEditorEvent::LagChange => FileEditorEvent::LagChange,
                    _ => FileEditorEvent::None
                }
            },
            FileEditor::JS(re) => {
                match re.handle_js_editor(cx, event, text_buffer) {
                    CodeEditorEvent::Change => FileEditorEvent::Change,
                    CodeEditorEvent::LagChange => FileEditorEvent::LagChange,
                    _ => FileEditorEvent::None
                }
            },
        }
    }
    
    pub fn set_key_focus(&mut self, cx: &mut Cx) {
        match self {
            FileEditor::Rust(re) => re.code_editor.set_key_focus(cx),
            FileEditor::JS(re) => re.code_editor.set_key_focus(cx),
        }
    }
    
    pub fn draw_file_editor(&mut self, cx: &mut Cx, text_buffer: &mut TextBuffer) {
        match self {
            FileEditor::Rust(re) => re.draw_rust_editor(cx, text_buffer),
            FileEditor::JS(re) => re.draw_js_editor(cx, text_buffer),
        }
    }
    
    pub fn create_file_editor_for_path(path: &str, template: &FileEditorTemplates) -> FileEditor {
        // check which file extension we have to spawn a new editor
        if path.ends_with(".rs") {
            FileEditor::Rust(RustEditor {
                ..template.rust_editor.clone()
            })
        }
        else if path.ends_with(".js") {
            FileEditor::JS(JSEditor {
                ..template.js_editor.clone()
            })
        }
        else {
            FileEditor::Rust(RustEditor {
                ..template.rust_editor.clone()
            })
        }
    }
}

pub fn path_file_name(path: &str) -> String {
    if let Some(pos) = path.rfind('/') {
        path[pos + 1..path.len()].to_string()
    }
    else {
        path.to_string()
    }
}
